//! C-ABI FFI layer for Android / JNI integration.
//!
//! Exposes a minimal surface so that Kotlin (or any other JNI consumer) can:
//! 1. Load the inference engine (`nihostt_engine_new`).
//! 2. Transcribe a WAV file (`nihostt_transcribe_file`).
//! 3. Stream audio in real-time (`nihostt_stream_new`, `nihostt_stream_process_chunk`,
//!    `nihostt_stream_flush`).
//! 4. Free the returned C string (`nihostt_string_free`).
//! 5. Tear down the engine (`nihostt_engine_free`).
//!
//! All functions are `unsafe` by nature (raw pointers cross the FFI boundary) but
//! the implementation checks nulls and logs errors before returning sentinel values.

use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::inference::{BlockingSessionGuard, Engine, StreamingSession};

/// Opaque handle to the inference engine.
///
/// The Kotlin side sees this as a `Long` (pointer-sized integer).
pub struct NihosttEngine {
    engine: Engine,
    disposed: AtomicBool,
}

/// Opaque handle to a streaming transcription session.
///
/// Holds a checked-out session from the pool and a `StreamingSession`. The session
/// is returned to the pool when `nihostt_stream_free` is called.
pub struct NihosttStream {
    session: BlockingSessionGuard,
    streaming: StreamingSession,
    disposed: AtomicBool,
}

/// Load the ONNX models from `model_dir` and create an inference engine.
///
/// Uses the default pool size (4). For mobile devices, prefer
/// `nihostt_engine_new_with_pool_size` with `pool_size = 1` to reduce RAM.
///
/// # Safety
/// `model_dir` must be a valid, null-terminated UTF-8 string.
/// Returns a pointer to a `NihosttEngine` on success, or `NULL` on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_engine_new(model_dir: *const c_char) -> *mut NihosttEngine {
    unsafe { nihostt_engine_new_with_pool_size(model_dir, 4) }
}

/// Load the ONNX models with a custom session pool size.
///
/// `pool_size` controls how many concurrent inference sessions are kept in
/// memory. Each session loads the full encoder, so RAM scales linearly:
/// - pool_size = 1: ~350 MB (recommended for mobile)
/// - pool_size = 4: ~560 MB (default desktop/server)
///
/// # Safety
/// `model_dir` must be a valid, null-terminated UTF-8 string.
/// Returns a pointer to a `NihosttEngine` on success, or `NULL` on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_engine_new_with_pool_size(
    model_dir: *const c_char,
    pool_size: usize,
) -> *mut NihosttEngine {
    if model_dir.is_null() {
        tracing::error!("nihostt_engine_new_with_pool_size: model_dir is null");
        eprintln!("nihostt_engine_new_with_pool_size: model_dir is null");
        return ptr::null_mut();
    }

    let dir_str = match unsafe { CStr::from_ptr(model_dir) }.to_str() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("nihostt_engine_new_with_pool_size: model_dir is not valid UTF-8: {e}");
            eprintln!("nihostt_engine_new_with_pool_size: model_dir is not valid UTF-8: {e}");
            return ptr::null_mut();
        }
    };

    match Engine::load_with_pool_size(dir_str, pool_size) {
        Ok(engine) => {
            let handle = Box::new(NihosttEngine {
                engine,
                disposed: AtomicBool::new(false),
            });
            Box::into_raw(handle)
        }
        Err(e) => {
            tracing::error!("nihostt_engine_new_with_pool_size: failed to load engine: {e}");
            eprintln!("nihostt_engine_new_with_pool_size: failed to load engine: {e}");
            ptr::null_mut()
        }
    }
}

/// Transcribe an audio file and return the recognized text as a newly allocated C string.
///
/// # Safety
/// - `engine` must be a non-null pointer returned by `nihostt_engine_new` and not yet freed.
/// - `audio_path` must be a valid, null-terminated UTF-8 string.
///
/// Returns a pointer to a NUL-terminated UTF-8 string on success, or `NULL` on failure.
/// The caller **must** free the returned string with `nihostt_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_transcribe_file(
    engine: *mut NihosttEngine,
    audio_path: *const c_char,
) -> *mut c_char {
    if engine.is_null() {
        tracing::error!("nihostt_transcribe_file: engine is null");
        return ptr::null_mut();
    }
    if audio_path.is_null() {
        tracing::error!("nihostt_transcribe_file: audio_path is null");
        return ptr::null_mut();
    }

    let path_str = match unsafe { CStr::from_ptr(audio_path) }.to_str() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("nihostt_transcribe_file: audio_path is not valid UTF-8: {e}");
            return ptr::null_mut();
        }
    };

    // Path sanitization: reject absolute paths and parent-dir traversal.
    let path = std::path::Path::new(path_str);
    if path.is_absolute() {
        tracing::error!("nihostt_transcribe_file: absolute paths are not allowed");
        return ptr::null_mut();
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        tracing::error!("nihostt_transcribe_file: paths containing '..' are not allowed");
        return ptr::null_mut();
    }

    let engine_ref = unsafe { &(*engine).engine };

    let mut guard = match engine_ref.pool.checkout_blocking() {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("nihostt_transcribe_file: failed to checkout session from pool: {e}");
            return ptr::null_mut();
        }
    };

    let result = match engine_ref.transcribe_file(path_str, guard.session()) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("nihostt_transcribe_file: transcription failed: {e}");
            return ptr::null_mut();
        }
    };

    match CString::new(result.text) {
        Ok(cstr) => cstr.into_raw(),
        Err(e) => {
            tracing::error!("nihostt_transcribe_file: result contains interior NUL: {e}");
            ptr::null_mut()
        }
    }
}

/// Free a C string previously returned by `nihostt_transcribe_file` or the
/// streaming functions.
///
/// # Safety
/// `s` must be a pointer returned by one of the transcription functions and not
/// yet freed, or `NULL` (in which case this is a no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_string_free(s: *mut c_char) {
    if !s.is_null() {
        let _ = unsafe { CString::from_raw(s) };
    }
}

/// Free an inference engine previously created by `nihostt_engine_new`.
///
/// # Safety
/// `engine` must be a pointer returned by `nihostt_engine_new` and not yet freed,
/// or `NULL` (in which case this is a no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_engine_free(engine: *mut NihosttEngine) {
    if !engine.is_null() {
        let disposed = unsafe { std::ptr::addr_of_mut!((*engine).disposed) };
        if unsafe { (*disposed).swap(true, Ordering::Relaxed) } {
            return;
        }
        let _ = unsafe { Box::from_raw(engine) };
    }
}

// ---------------------------------------------------------------------------
// Streaming API
// ---------------------------------------------------------------------------

/// Create a new streaming session.
///
/// Checks out a session from the engine pool and creates a fresh
/// `StreamingSession`. The session is held for the lifetime of the stream and
/// returned to the pool by `nihostt_stream_free`.
///
/// # Safety
/// `engine` must be a valid pointer returned by `nihostt_engine_new`.
/// Returns a pointer to a `NihosttStream` on success, or `NULL` on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_stream_new(engine: *mut NihosttEngine) -> *mut NihosttStream {
    if engine.is_null() {
        tracing::error!("nihostt_stream_new: engine is null");
        return ptr::null_mut();
    }

    let engine_ref = unsafe { &(*engine).engine };

    let session = match engine_ref.pool.checkout_blocking() {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("nihostt_stream_new: pool checkout failed: {e}");
            return ptr::null_mut();
        }
    };

    let streaming = match engine_ref.create_streaming_session() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("nihostt_stream_new: streaming session creation failed: {e}");
            return ptr::null_mut();
        }
    };
    let stream = NihosttStream {
        session,
        streaming,
        disposed: AtomicBool::new(false),
    };
    Box::into_raw(Box::new(stream))
}

/// Process a chunk of PCM16 audio and return any partial/final segments.
///
/// # Safety
/// - `engine` and `stream` must be valid pointers.
/// - `pcm16_bytes` must point to at least `len` valid bytes (little-endian mono PCM16).
///
/// Returns a newly allocated JSON array string on success, or `NULL` on failure.
/// The caller **must** free the returned string with `nihostt_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_stream_process_chunk(
    _engine: *mut NihosttEngine,
    stream: *mut NihosttStream,
    pcm16_bytes: *const u8,
    len: usize,
    sample_rate: u32,
) -> *mut c_char {
    if stream.is_null() {
        tracing::error!("nihostt_stream_process_chunk: stream is null");
        return ptr::null_mut();
    }
    if pcm16_bytes.is_null() {
        tracing::error!("nihostt_stream_process_chunk: pcm16_bytes is null");
        return ptr::null_mut();
    }

    let stream_ref = unsafe { &mut (*stream) };

    // Convert PCM16 LE bytes → f32 samples.
    let bytes = unsafe { std::slice::from_raw_parts(pcm16_bytes, len) };
    let pcm16: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let mut samples_f32: Vec<f32> = pcm16.iter().map(|&s| s as f32 / 32768.0).collect();

    // Resample to 16 kHz if needed.
    if sample_rate != 16000 {
        samples_f32 = match crate::inference::audio::resample(&samples_f32, sample_rate, 16000) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("nihostt_stream_process_chunk: resample failed: {e}");
                return ptr::null_mut();
            }
        };
    }

    let results = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        stream_ref
            .streaming
            .process_chunk(&samples_f32, &mut stream_ref.session)
    })) {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => {
            tracing::error!("nihostt_stream_process_chunk: inference failed: {e}");
            return ptr::null_mut();
        }
        Err(_) => {
            tracing::error!("nihostt_stream_process_chunk: panic during inference");
            return ptr::null_mut();
        }
    };

    let json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".into());
    match CString::new(json) {
        Ok(cstr) => cstr.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Flush the streaming state and return the final segment(s).
///
/// # Safety
/// `stream` must be a valid pointer.
///
/// Returns a newly allocated JSON array string (possibly `[]`) on success,
/// or `NULL` on failure. The caller **must** free the returned string with
/// `nihostt_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_stream_flush(stream: *mut NihosttStream) -> *mut c_char {
    if stream.is_null() {
        tracing::error!("nihostt_stream_flush: stream is null");
        return ptr::null_mut();
    }

    let stream_ref = unsafe { &mut (*stream) };

    let result = match stream_ref.streaming.finalize(&mut stream_ref.session) {
        Ok(results) => results,
        Err(e) => {
            tracing::error!("nihostt_stream_flush: inference failed: {e}");
            return ptr::null_mut();
        }
    };

    let json = serde_json::to_string(&result).unwrap_or_else(|_| "[]".into());
    match CString::new(json) {
        Ok(cstr) => cstr.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Free a streaming session and return its session to the pool.
///
/// # Safety
/// `stream` must be a pointer returned by `nihostt_stream_new` and not yet freed,
/// or `NULL` (in which case this is a no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nihostt_stream_free(stream: *mut NihosttStream) {
    if !stream.is_null() {
        let disposed = unsafe { std::ptr::addr_of_mut!((*stream).disposed) };
        if unsafe { (*disposed).swap(true, Ordering::Relaxed) } {
            return;
        }
        let _ = unsafe { Box::from_raw(stream) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_stream_new_null_engine() {
        let stream = unsafe { nihostt_stream_new(ptr::null_mut()) };
        assert!(stream.is_null());
    }

    #[test]
    fn test_stream_process_chunk_null_args() {
        let r = unsafe {
            nihostt_stream_process_chunk(ptr::null_mut(), ptr::null_mut(), ptr::null(), 0, 16000)
        };
        assert!(r.is_null());
    }

    #[test]
    fn test_stream_flush_null_args() {
        let r = unsafe { nihostt_stream_flush(ptr::null_mut()) };
        assert!(r.is_null());
    }

    #[test]
    fn test_stream_free_null() {
        // Should be a no-op, not a crash.
        unsafe { nihostt_stream_free(ptr::null_mut()) };
    }

    #[test]
    fn test_engine_new_null_dir() {
        let engine = unsafe { nihostt_engine_new(ptr::null()) };
        assert!(engine.is_null());
    }

    #[test]
    fn test_engine_new_invalid_utf8() {
        let bad = [0x80u8, 0x81, 0x82, 0];
        let engine = unsafe { nihostt_engine_new(bad.as_ptr() as *const c_char) };
        assert!(engine.is_null());
    }

    #[test]
    #[ignore] // Requires model download.
    fn test_stream_free_returns_session_to_pool() {
        let model_dir = crate::model::default_model_dir();
        let encoder = std::path::Path::new(&model_dir).join("encoder-epoch-99-avg-1.onnx");
        assert!(
            encoder.exists(),
            "Model not found at {}. Run `cargo run -- download` first.",
            model_dir
        );

        let model_dir = CString::new(model_dir).expect("model dir has no NUL");
        let engine = unsafe { nihostt_engine_new_with_pool_size(model_dir.as_ptr(), 1) };
        assert!(!engine.is_null(), "engine should load with pool_size=1");

        let first = unsafe { nihostt_stream_new(engine) };
        assert!(
            !first.is_null(),
            "first stream should checkout the only session"
        );
        unsafe { nihostt_stream_free(first) };

        let second = unsafe { nihostt_stream_new(engine) };
        assert!(
            !second.is_null(),
            "freeing a stream must return its session to the pool"
        );

        unsafe {
            nihostt_stream_free(second);
            nihostt_engine_free(engine);
        }
    }
}
