# Multi-stage build for nihostt
# Build: docker build -t nihostt .
# Run:   docker run -p 9876:9876 nihostt

# --- Builder stage ---
FROM rust:1.85-bookworm AS builder

# `prost-build` (via build.rs) requires `protoc` at compile time; without it
# the build aborts with "prost-build failed to compile proto/onnx.proto".
RUN apt-get update && \
    apt-get install -y --no-install-recommends protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Dependency-compilation cache: copy manifests + build.rs + proto/ first and
# compile a dummy binary so `cargo build` downloads + builds every transitive
# crate. Subsequent edits to src/ only invalidate the final compilation
# layer, cutting incremental rebuild time from minutes to seconds.
COPY Cargo.toml Cargo.lock build.rs ./
COPY proto/ proto/
RUN mkdir -p src tests && \
    echo 'fn main() {}' > src/main.rs && \
    touch src/lib.rs && \
    echo '' > tests/benchmark.rs && \
    cargo build --release && \
    rm -rf src tests target/release/deps/nihostt-* target/release/nihostt*

# Now bring in the actual source and build the real binary.
COPY src/ src/

RUN cargo build --release && \
    strip target/release/nihostt

# --- Model bake stage (runs only when NIHOSTT_BAKE_MODEL=1) ---
FROM debian:bookworm-slim AS model-fetcher

ARG NIHOSTT_BAKE_MODEL=0

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/nihostt /usr/local/bin/nihostt

RUN mkdir -p /models && \
    if [ "$NIHOSTT_BAKE_MODEL" = "1" ]; then \
        nihostt download --model-dir /models; \
    fi

# --- Runtime stage ---
FROM debian:bookworm-slim

ARG NIHOSTT_BAKE_MODEL=0

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/nihostt /usr/local/bin/nihostt

RUN groupadd -r nihostt && useradd -r -g nihostt nihostt && \
    mkdir -p /home/nihostt/.nihostt/models && chown -R nihostt:nihostt /home/nihostt

# Copy baked model files (only present when NIHOSTT_BAKE_MODEL=1)
COPY --from=model-fetcher --chown=nihostt:nihostt /models/. /home/nihostt/.nihostt/models/

USER nihostt

ENV RUST_LOG=nihostt=info

EXPOSE 9876

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:9876/health || exit 1

# Download model if not present, then start server.
# `--bind-all` acknowledges that container networking requires listening on
# 0.0.0.0; outside Docker the default `127.0.0.1` bind stays in effect.
ENTRYPOINT ["nihostt"]
CMD ["serve", "--port", "9876", "--host", "0.0.0.0", "--bind-all"]
