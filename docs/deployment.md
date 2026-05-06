# Deployment Guide

## Docker

### CPU (any platform)

```bash
docker build -t nihostt .
docker run -p 9876:9876 -v ~/.nihostt/models:/root/.nihostt/models nihostt
```

### CUDA (Linux + NVIDIA)

```bash
docker build -f Dockerfile.cuda -t nihostt:cuda .
docker run --gpus all -p 9876:9876 -v ~/.nihostt/models:/root/.nihostt/models nihostt:cuda
```

### Pre-built image with model baked in

```bash
docker build --build-arg NIHOSTT_BAKE_MODEL=1 -t nihostt:baked .
docker run -p 9876:9876 nihostt:baked
```

## systemd Service

Create `/etc/systemd/system/nihostt.service`:

```ini
[Unit]
Description=nihostt Japanese STT server
After=network.target

[Service]
Type=simple
User=nihostt
ExecStart=/usr/local/bin/nihostt serve --bind-all --host 0.0.0.0 --port 9876 --allow-origin https://stt.example.com --metrics
Restart=on-failure
Environment="RUST_LOG=nihostt=info"

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now nihostt
```

## Reverse Proxy (nginx)

```nginx
server {
    listen 443 ssl;
    server_name stt.example.com;

    location / {
        proxy_pass http://127.0.0.1:9876;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```

Start nihostt with `--allow-origin https://stt.example.com` for browser clients.
Use `--trust-proxy` only when the reverse proxy is trusted to set
`X-Forwarded-For` / `X-Real-IP`; otherwise rate limiting uses the socket IP.

## Environment Variables

| Variable | CLI Flag | Default |
|---|---|---|
| `NIHOSTT_ALLOW_BIND_ANY` | `--bind-all` | — |
| `NIHOSTT_IDLE_TIMEOUT_SECS` | `--idle-timeout-secs` | 300 |
| `NIHOSTT_WS_FRAME_MAX_BYTES` | `--ws-frame-max-bytes` | 524288 |
| `NIHOSTT_BODY_LIMIT_BYTES` | `--body-limit-bytes` | 52428800 |
| `NIHOSTT_RATE_LIMIT_PER_MINUTE` | `--rate-limit-per-minute` | 60 |
| `NIHOSTT_RATE_LIMIT_BURST` | `--rate-limit-burst` | 10 |
| `NIHOSTT_MAX_SESSION_SECS` | `--max-session-secs` | 3600 |
| `NIHOSTT_SHUTDOWN_DRAIN_SECS` | `--shutdown-drain-secs` | 10 |
| `NIHOSTT_SKIP_QUANTIZE` | `--skip-quantize` | false |
| `NIHOSTT_METRICS` | `--metrics` | false |
| `NIHOSTT_TRUST_PROXY` | `--trust-proxy` | false |

`--allow-origin` is repeatable and currently has no environment-variable alias.
Loopback origins are always allowed. `--cors-allow-any` disables origin
restriction and should only be used behind a trusted boundary.
