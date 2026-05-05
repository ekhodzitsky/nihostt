# Runbook

Operator-facing guidance for running nihostt in production.

## Installation

### Binary

Download the latest release from [GitHub Releases](https://github.com/ekhodzitsky/nihostt/releases):

```sh
curl -sL https://github.com/ekhodzitsky/nihostt/releases/latest/download/nihostt-$(uname -m)-apple-darwin.tar.gz | tar xz
sudo mv nihostt /usr/local/bin/
```

### Docker

```sh
# CPU (any platform)
docker build -t nihostt .
docker run -p 9876:9876 -v ~/.nihostt/models:/root/.nihostt/models nihostt

# CUDA (Linux, requires NVIDIA Container Toolkit)
docker build -f Dockerfile.cuda -t nihostt:cuda .
docker run --gpus all -p 9876:9876 -v ~/.nihostt/models:/root/.nihostt/models nihostt:cuda
```

### Homebrew

```sh
brew tap ekhodzitsky/nihostt
brew install nihostt
```

## Starting the server

```sh
# Download model first (~155 MB INT8, one-time)
nihostt download

# Start server (listens on 127.0.0.1:9876 by default)
nihostt serve

# With custom options
nihostt serve --port 8080 --pool-size 2 --metrics
```

## Checking health

```sh
curl http://127.0.0.1:9876/health
```

Expected response:

```json
{
  "status": "ok",
  "model": "reazonspeech-k2-v2",
  "language": "ja"
}
```

## Logs and metrics

### Logs

Set log level via `--log-level` or `RUST_LOG`:

```sh
RUST_LOG=nihostt=debug nihostt serve
```

### Prometheus metrics

Enable with `--metrics` (or `NIHOSTT_METRICS=1`):

```sh
nihostt serve --metrics
```

Scrape at `GET /metrics`:

```
# HELP nihostt_http_requests_total Total HTTP requests
# TYPE nihostt_http_requests_total counter
nihostt_http_requests_total{path="/health",status="200"} 42
```

## Common issues

### Model not found

**Symptom:** Server exits with "model file not found" or similar.

**Fix:** Run `nihostt download` to fetch the model files:

```sh
nihostt download
```

### Bind errors

**Symptom:** `refusing to bind to '0.0.0.0': non-loopback addresses require --bind-all`

**Fix:** Use `--bind-all` or set `NIHOSTT_ALLOW_BIND_ANY=1`:

```sh
nihostt serve --bind-all --host 0.0.0.0
# or
NIHOSTT_ALLOW_BIND_ANY=1 nihostt serve --host 0.0.0.0
```

Required for Docker and reverse-proxy deployments.

### OOM / high memory usage

**Symptom:** Process killed by OOM killer or RSS grows unbounded.

**Fix:** Reduce `--pool-size` (default 4). Each session loads the full ONNX model into memory:

```sh
nihostt serve --pool-size 2
```

Expected memory: ~350 MB RSS with pool-size 1, scaling roughly linearly.

### WebSocket idle disconnects

**Symptom:** Clients disconnected after periods of silence.

**Fix:** Increase `--idle-timeout-secs` (default 300 s):

```sh
nihostt serve --idle-timeout-secs 600
```

## Upgrading

1. Download the new binary or pull the new Docker image.
2. Stop the running server (SIGTERM triggers graceful shutdown).
3. Restart with the new binary.

Model files are backward-compatible across patch releases. If a new model version is required, the server will log a warning on startup — run `nihostt download` to update.

## On-call triage checklist

1. Check `/health` — should return `{"status":"ok"}`.
2. Check server logs for `process_chunk error` or `pool saturated`.
3. If pool is saturated, check current load and consider scaling `--pool-size` or adding instances.
4. Verify model files exist in `~/.nihostt/models/`:
   - `encoder-epoch-99-avg-1.onnx`
   - `decoder-epoch-99-avg-1.onnx`
   - `joiner-epoch-99-avg-1.onnx`
   - `tokens.txt`