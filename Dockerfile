# Multi-stage build for nihostt
# Build: docker build -t nihostt .
# Run:   docker run -p 9876:9876 nihostt

# --- Builder stage ---
# Ubuntu 24.04 provides glibc 2.39, required by prebuilt ONNX Runtime
# binaries downloaded by ort (which reference __isoc23_* symbols).
FROM ubuntu:24.04 AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates curl build-essential pkg-config \
        protobuf-compiler libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Install Rust toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain 1.93.0
ENV PATH="/root/.cargo/bin:${PATH}"

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

RUN mkdir -p tests && echo '' > tests/benchmark.rs && \
    cargo build --release && \
    strip target/release/nihostt

# --- Model bake stage (runs only when NIHOSTT_BAKE_MODEL=1) ---
FROM ubuntu:24.04 AS model-fetcher

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
FROM ubuntu:24.04

ARG NIHOSTT_BAKE_MODEL=0

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/nihostt /usr/local/bin/nihostt

RUN groupadd --gid 10001 nihostt && \
    useradd --uid 10001 --gid nihostt --home-dir /home/nihostt \
        --create-home --shell /usr/sbin/nologin --no-log-init nihostt && \
    mkdir -p /home/nihostt/.nihostt/models && chown -R nihostt:nihostt /home/nihostt

# Copy baked model files (only present when NIHOSTT_BAKE_MODEL=1)
COPY --from=model-fetcher --chown=nihostt:nihostt /models/. /home/nihostt/.nihostt/models/

USER nihostt

ENV HOME=/home/nihostt
ENV RUST_LOG=nihostt=info

EXPOSE 9876

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:9876/health || exit 1

# Download model if not present, then start server. Container/public binds
# require NIHOSTT_API_KEYS unless --allow-unauthenticated-public is explicit.
# `--bind-all` acknowledges that container networking requires listening on
# 0.0.0.0; outside Docker the default `127.0.0.1` bind stays in effect.
ENTRYPOINT ["nihostt"]
CMD ["serve", "--port", "9876", "--host", "0.0.0.0", "--bind-all"]
