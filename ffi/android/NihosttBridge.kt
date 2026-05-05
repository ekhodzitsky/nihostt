package com.nihostt

/**
 * Kotlin JNI bridge for the nihostt Japanese STT engine.
 *
 * Load `libnihostt.so` from your app's `jniLibs` and call these functions
 * to run on-device speech recognition.
 *
 * Typical lifecycle:
 * ```
 * val engine = NihosttBridge.engineNew(modelDir.absolutePath)
 * if (engine == 0L) { /* handle error */ }
 *
 * val text = NihosttBridge.transcribeFile(engine, wavPath)
 * NihosttBridge.stringFree(text)   // only if you got a non-null string
 *
 * NihosttBridge.engineFree(engine)
 * ```
 */
object NihosttBridge {

    init {
        System.loadLibrary("nihostt")
    }

    /**
     * Load the ONNX models from [modelDir] and create an inference engine.
     *
     * Uses the default pool size (4). On mobile devices prefer
     * [engineNewWithPoolSize] with `poolSize = 1` to reduce RAM.
     *
     * Returns an opaque handle (pointer cast to Long) or 0L on failure.
     */
    @JvmStatic
    external fun engineNew(modelDir: String): Long

    /**
     * Load the ONNX models from [modelDir] with a custom session pool size.
     *
     * `poolSize` controls concurrent inference sessions. Each session loads
     * the full encoder, so RAM scales linearly:
     * - 1: ~350 MB (recommended for mobile)
     * - 4: ~560 MB (default desktop/server)
     *
     * Returns an opaque handle or 0L on failure.
     */
    @JvmStatic
    external fun engineNewWithPoolSize(modelDir: String, poolSize: Int): Long

    /**
     * Quantize the FP32 encoder to INT8 on-device.
     *
     * Looks for `encoder-epoch-99-avg-1.onnx` inside [modelDir] and produces
     * `encoder-epoch-99-avg-1.int8.onnx` in the same directory.
     * If the INT8 file already exists and [force] is false, returns immediately.
     *
     * Returns `"ok"` on success, or an error message on failure.
     * The returned string must be freed with [stringFree].
     */
    @JvmStatic
    external fun quantizeModel(modelDir: String, force: Boolean): String

    /**
     * Transcribe a WAV file and return the recognized text.
     *
     * The returned string must be freed with [stringFree] when no longer needed.
     * Returns `null` on error.
     */
    @JvmStatic
    external fun transcribeFile(engine: Long, audioPath: String): String?

    /**
     * Create a new real-time streaming session.
     *
     * Returns an opaque stream handle or 0L on failure.
     */
    @JvmStatic
    external fun streamNew(engine: Long): Long

    /**
     * Feed a chunk of PCM16 audio into a streaming session.
     *
     * [pcm16Bytes] must be little-endian mono PCM16 at the given [sampleRate].
     * The audio is resampled to 16 kHz internally if needed.
     *
     * Returns a JSON array of transcript results (or `null` on error).
     * Each element is either `{"Partial":"..."}` or `{"Final":"..."}`.
     * The returned string must be freed with [stringFree].
     */
    @JvmStatic
    external fun streamProcessChunk(
        engine: Long,
        stream: Long,
        pcm16Bytes: ByteArray,
        sampleRate: Int
    ): String?

    /**
     * Signal end-of-stream and return the final segment(s).
     *
     * Returns a JSON array (possibly `[]`). The string must be freed with [stringFree].
     */
    @JvmStatic
    external fun streamFlush(stream: Long): String?

    /**
     * Free a streaming session and return its inference session to the pool.
     */
    @JvmStatic
    external fun streamFree(stream: Long)

    /**
     * Free a C string previously returned by [transcribeFile], [streamProcessChunk],
     * or [streamFlush].
     */
    @JvmStatic
    external fun stringFree(s: String?)

    /**
     * Tear down the engine and release all ONNX sessions.
     */
    @JvmStatic
    external fun engineFree(engine: Long)
}
