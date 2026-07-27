FROM rust:1.97-slim AS builder

WORKDIR /usr/src/app

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

FROM ubuntu:24.04 AS runner

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/src/app/target/release/amll-ttml-api /app/amll-ttml-api

ENV PORT=41856
EXPOSE 41856

CMD ["/app/amll-ttml-api"]
