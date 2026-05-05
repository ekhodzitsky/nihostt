# Android FFI Guide

> On-device Japanese speech-to-text for Android apps, powered by nihostt's Rust inference engine.

---

## Overview

nihostt's inference engine (`nihostt::inference::Engine`) is pure Rust and has no dependency on the server stack (`axum`, `tokio` runtime, etc.). This makes it an ideal candidate for on-device STT inside Android applications — no network latency, no cloud API keys, and full privacy.

This document describes the architecture, build process, and integration steps needed to ship nihostt as a native library (`libnihostt.so`) inside an Android app.

---

## Architecture

```
┌─────────────────────────────────────────┐
│  Kotlin / Android App                   │
│  (UI, microphone, file picker)          │
├─────────────────────────────────────────┤
│  NihosttBridge.kt  ──►  JNI            │
│  (object with external fun declarations)│
├─────────────────────────────────────────┤
│  libnihostt.so  ──►  C-ABI FFI         │
│  (src/ffi.rs)                           │
├─────────────────────────────────────────┤
│  nihostt::inference::Engine             │
│  (ONNX Runtime + ReazonSpeech-k2-v2)    │
└─────────────────────────────────────────┘
```

1. **Kotlin layer** — thin bridge that loads `libnihostt.so` and calls the exported functions.
2. **JNI glue** — generated headers from `javac -h` (or `cargo-jni`). The Kotlin signatures map directly to the C symbols in `src/ffi.rs`.
3. **Rust FFI layer** — `src/ffi.rs` exposes:
   - `nihostt_engine_new(model_dir)` → opaque engine handle
   - `nihostt_engine_new_with_pool_size(model_dir, pool_size)` → opaque engine handle
   - `nihostt_transcribe_file(engine, audio_path)` → allocated C string
   - `nihostt_stream_new(engine)` → opaque stream handle
   - `nihostt_stream_process_chunk(engine, stream, pcm16_bytes, len, sample_rate)` → JSON results
   - `nihostt_stream_flush(stream)` → final JSON results
   - `nihostt_quantize_model(model_dir, force)` → allocated C string (`"ok"` or error)
   - `nihostt_string_free(s)` — frees the C string safely
   - `nihostt_stream_free(stream)` — frees stream
   - `nihostt_engine_free(engine)` — tears down the engine
4. **Inference engine** — `Engine::load` reads ONNX models from disk, `transcribe_file` runs the full encoder + decoder + joiner pipeline.

---

## Prerequisites

- **Android NDK** (r26c or newer recommended). Download via Android Studio SDK Manager or from [developer.android.com/ndk](https://developer.android.com/ndk).
- **Rust toolchain** with Android targets:
  ```sh
  rustup target add aarch64-linux-android
  rustup target add armv7-linux-androideabi
  ```
- **cargo-ndk** — Cargo helper that sets up the NDK compiler and linker:
  ```sh
  cargo install cargo-ndk
  ```
- Ensure `$NDK_HOME` (or `$ANDROID_NDK_HOME`) points to your NDK root.

---

## Building the Rust Library for Android

### 1. Configure the linker

Create `.cargo/config.toml` in the project root:

```toml
[target.aarch64-linux-android]
linker = "aarch64-linux-android35-clang"

[target.armv7-linux-androideabi]
linker = "armv7a-linux-androideabi35-clang"
```

> **Adjust the API level** (`35`) to match your installed NDK version and your app's `minSdkVersion`.

### 2. Build with cargo-ndk

```sh
cargo ndk -t arm64-v8a -o ./android/app/src/main/jniLibs build --release --features ffi
```

For multiple architectures:

```sh
cargo ndk \
  -t arm64-v8a \
  -t armeabi-v7a \
  -t x86_64 \
  -o ./android/app/src/main/jniLibs \
  build --release --features ffi
```

The resulting `.so` files land in:

```
android/app/src/main/jniLibs/
├── arm64-v8a/libnihostt.so
├── armeabi-v7a/libnihostt.so
└── x86_64/libnihostt.so
```

Gradle packages these automatically into the APK/AAB.

---

## Model Bundling

The ReazonSpeech-k2-v2 INT8 model set is ~155 MB on disk (FP32 is ~590 MB):

| File | Size (approx) |
|------|---------------|
| `encoder-epoch-99-avg-1.onnx` (INT8) | ~155 MB |
| `encoder-epoch-99-avg-1.onnx` (FP32) | ~590 MB |
| `decoder-epoch-99-avg-1.onnx` | ~4.4 MB |
| `joiner-epoch-99-avg-1.onnx` | ~2.6 MB |
| `tokens.txt` | ~45 KB |
| `silero_vad.onnx` | ~2.3 MB |

> **Always use the INT8 encoder on mobile.** The FP32 encoder will OOM on most devices.

You have two strategies for shipping models:

### A. Bundle in `assets/` (simplest)

1. Copy the contents of `~/.nihostt/models/` into:
   ```
   android/app/src/main/assets/nihostt_models/
   ```
2. On first app launch, copy the files from assets to the app's private storage:
   ```kotlin
   val assetManager = context.assets
   val modelDir = File(context.filesDir, "nihostt_models")
   copyAssets(assetManager, "nihostt_models", modelDir)
   ```
3. Pass `modelDir.absolutePath` to `NihosttBridge.engineNew(...)`.

### B. Download on first run (smaller APK)

1. Ship a tiny stub APK.
2. On first launch, download the model files from your own CDN or HuggingFace.
3. Extract to the app's private files directory.

> ⚠️ The total APK size with bundled assets is ~165+ MB. Google Play supports up to 200 MB for APKs and larger sizes via App Bundles, but consider using [Play Feature Delivery](https://developer.android.com/guide/playcore/feature-delivery) or download-on-demand for the model files.

---

## ONNX Runtime on Android

The `ort` crate supports Android through multiple execution providers:

- **NNAPI** (Neural Networks API) — uses the device's NPU / DSP when available. Enabled automatically when you build with `--features ffi` because the `ffi` feature pulls in `ort/nnapi`.
- **CPU** — pure CPU fallback, works on every device. This is the default when NNAPI is unavailable or fails to initialize.

No code changes are required to switch between EPs; `ort` selects the best available provider at session creation time.

---

## Kotlin / JNI Bridge Skeleton

The skeleton lives at [`ffi/android/NihosttBridge.kt`](ffi/android/NihosttBridge.kt). A typical usage flow looks like this:

```kotlin
class MainActivity : AppCompatActivity() {

    private var enginePtr: Long = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        val modelDir = File(filesDir, "nihostt_models")
        enginePtr = NihosttBridge.engineNew(modelDir.absolutePath)
        if (enginePtr == 0L) {
            Toast.makeText(this, "Failed to load STT engine", Toast.LENGTH_LONG).show()
            return
        }
    }

    fun transcribe(wavPath: String): String {
        if (enginePtr == 0L) return ""
        return NihosttBridge.transcribeFile(enginePtr, wavPath)
    }

    override fun onDestroy() {
        if (enginePtr != 0L) {
            NihosttBridge.engineFree(enginePtr)
            enginePtr = 0L
        }
        super.onDestroy()
    }
}
```

---

## On-device Quantization

Instead of bundling or downloading the ~155 MB INT8 encoder, you can ship the ~590 MB FP32 encoder and quantize it on the device after download. This is a one-time operation that takes about 2 minutes on a modern flagship phone and produces `encoder-epoch-99-avg-1.int8.onnx` in the same directory.

```kotlin
val modelDir = File(context.filesDir, "nihostt_models")

// Run once after the FP32 model is downloaded.
val result = NihosttBridge.quantizeModel(modelDir.absolutePath, force = false)
if (result != "ok") {
    Log.e("NihoSTT", "Quantization failed: $result")
}
NihosttBridge.stringFree(result)
```

> **Why?** If your backend already stores the FP32 model, you can avoid maintaining a separate INT8 artifact. The quantization is deterministic, so every device produces the same INT8 weights.

## Size Considerations

| Component | Approximate Size |
|-----------|------------------|
| `libnihostt.so` (arm64, stripped, release, LTO) | ~20–30 MB |
| ONNX models (INT8) | ~155 MB |
| **Total on-device** | ~175–185 MB |

Tips to reduce binary size:

- `strip = true` and `lto = true` are already enabled in `Cargo.toml` for release builds.
- Build only for `arm64-v8a` if you do not need 32-bit ARM support.
- Use `cargo-ndk` with `--release` (debug builds are much larger).
- Use `--features ffi` (excludes server code paths where possible).
- Consider downloading models at runtime instead of bundling.

---

## Current Limitations

1. **Server code is excluded from FFI** — The FFI build only exposes `nihostt::inference::Engine`. The WebSocket server, REST handlers, rate limiting, and `tokio` runtime with `rt-multi-thread` are compiled out when used as a library. They still exist in the server binary (`cargo build --bin nihostt`).

2. **Synchronous transcription only** — `nihostt_transcribe_file` blocks the calling thread while inference runs. Call it from a Kotlin coroutine (`withContext(Dispatchers.IO)`) so the UI thread stays responsive.

3. **No Java exception translation** — Rust errors are logged and returned as `NULL` / empty string. The Kotlin side should treat `engineNew == 0L` or `transcribeFile == ""` as failure and surface a generic error message.

4. **Model directory layout is fixed** — `Engine::load` expects exactly the filenames from the download (`encoder-epoch-99-avg-1.onnx`, `decoder-epoch-99-avg-1.onnx`, `joiner-epoch-99-avg-1.onnx`, `tokens.txt`). Do not rename them.

5. **Memory footprint** — ~500 MB RSS with default pool size 4. On mobile, use `pool_size = 1` to reduce RAM to ~350 MB.

---

## Troubleshooting

### `cargo-ndk` cannot find the linker

Make sure `$NDK_HOME` is set and the NDK version in `.cargo/config.toml` matches your installed NDK. Example:

```sh
export NDK_HOME=$HOME/Android/Sdk/ndk/26.1.10909125
```

Then update `.cargo/config.toml`:

```toml
[target.aarch64-linux-android]
linker = "aarch64-linux-android26-clang"
```

### `ort` fails to compile for Android

Ensure you added the Android Rust targets:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi
```

If you see linker errors about missing `libclang_rt.builtins`, your NDK installation may be incomplete. Re-install the NDK via Android Studio.

### NNAPI is not used at runtime

Check `adb logcat` for `ort` messages. NNAPI requires:
- Android API 27+ (preferably 28+)
- A device with a compatible NPU / DSP driver
- The model ops must be supported by NNAPI (some ops fall back to CPU automatically)

This is safe — inference still works, just on CPU.

### Out of memory on model load

- Ensure you are using the INT8 encoder, not the FP32 one.
- Reduce pool size to 1 by modifying `Engine::load` call or using a custom build.
- Close other apps before testing on low-RAM devices.

---

*Last updated: 2026-05-05*
