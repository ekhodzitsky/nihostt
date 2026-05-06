# CLI Reference

Complete command-line interface for `nihostt`.

All flags have corresponding environment variables (see individual options below).

```
nihostt [OPTIONS] <COMMAND>

Options:
  --log-level <LEVEL>    Log level [default: info]

Commands:
  serve        Start STT server
  download     Download model (~155 MB INT8)
  quantize     Quantize encoder to INT8 (or download pre-quantized)
  transcribe   Transcribe audio file (offline)

nihostt serve [OPTIONS]
  --port <PORT>             Listen port [default: 9876]
  --host <HOST>             Bind address [default: 127.0.0.1]
  --model-dir <DIR>         Model directory [default: ~/.nihostt/models]
  --pool-size <N>           Concurrent inference sessions [default: 4]
  --bind-all                Required to listen on a non-loopback address.
                            Also: NIHOSTT_ALLOW_BIND_ANY=1.
  --allow-origin <URL>      Additional Origin allowed (repeatable).
                            Loopback origins are always allowed.
  --cors-allow-any          Accept any cross-origin caller (wildcard CORS).
  --idle-timeout-secs <S>   WebSocket idle timeout [default: 300].
                            Env: NIHOSTT_IDLE_TIMEOUT_SECS.
  --ws-frame-max-bytes <B>  Max WS frame size [default: 524288 = 512 KiB].
                            Env: NIHOSTT_WS_FRAME_MAX_BYTES.
  --body-limit-bytes <B>    Max REST body size [default: 52428800 = 50 MiB].
                            Env: NIHOSTT_BODY_LIMIT_BYTES.
  --rate-limit-per-minute <N>  Per-IP rate limit (requests/min). Default: 60.
                            Set to 0 to disable. Applies to /v1/* only;
                            /health, /ready, and /metrics are exempt.
                            Env: NIHOSTT_RATE_LIMIT_PER_MINUTE.
  --rate-limit-burst <N>    Token-bucket burst size [default: 10].
                            Env: NIHOSTT_RATE_LIMIT_BURST.
  --metrics                 Expose Prometheus metrics at GET /metrics.
                            Off by default. Env: NIHOSTT_METRICS.
  --max-session-secs <S>        Wall-clock session cap [default: 3600]. 0 = disabled.
                                Env: NIHOSTT_MAX_SESSION_SECS.
  --shutdown-drain-secs <S>     Max wait for in-flight sessions on SIGTERM [default: 10].
                                Env: NIHOSTT_SHUTDOWN_DRAIN_SECS.
  --trust-proxy               Trust X-Forwarded-For and X-Real-IP for rate-limit IP extraction.
                                Env: NIHOSTT_TRUST_PROXY.
  --skip-quantize               Skip auto-quantization step on first run.
                                Env: NIHOSTT_SKIP_QUANTIZE.

nihostt download [OPTIONS]
  --model-dir <DIR>      Model directory [default: ~/.nihostt/models]
  --skip-quantize        Skip auto-quantization after download (FP32 only)
                         Env: NIHOSTT_SKIP_QUANTIZE.

nihostt quantize [OPTIONS]
  --model-dir <DIR>      Model directory [default: ~/.nihostt/models]
  --force                Force re-quantization even if INT8 model exists

nihostt transcribe [OPTIONS] <FILE>
  --model-dir <DIR>      Model directory [default: ~/.nihostt/models]
  Supports: WAV, M4A, MP3, OGG, FLAC (mono or auto-mixed)
```
