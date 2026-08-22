FROM rust:1.97-slim AS builder

WORKDIR /usr/src/app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src

RUN cargo build --release

FROM ubuntu:24.04 AS runner

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/src/app/target/release/amll-ttml-api /app/amll-ttml-api

RUN mkdir -p /data

ENV PORT=3000 \
    DATABASE_URL="sqlite:///data/amll_lyrics.db?mode=rwc"

EXPOSE 3000
VOLUME ["/data"]

ENTRYPOINT ["/app/amll-ttml-api"]
