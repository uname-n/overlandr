FROM rust:1.88-slim-bookworm AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y \
    pkg-config libzstd-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies layer
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

COPY . .
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    libzstd1 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/overlandr /usr/local/bin/overlandr

ARG STATE
COPY bin/${STATE}.bin /data/graph.bin

LABEL org.opencontainers.image.source=https://github.com/uname-n/overlandr
LABEL org.opencontainers.image.url=https://github.com/uname-n/overlandr
LABEL org.opencontainers.image.title="overlandr (${STATE})"
LABEL org.opencontainers.image.description="Overland route planning server for ${STATE}."

ENV GRAPH_PATH=/data/graph.bin
ENV PORT=3000

EXPOSE 3000

CMD ["overlandr", "serve"]
