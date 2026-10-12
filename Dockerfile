# 阶段 1: 使用预装 cargo-chef 的基础镜像准备依赖缓存清单
FROM lukemathwalker/cargo-chef:latest-rust-1-slim AS chef
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 阶段 2: 分析依赖树并生成 recipe.json
FROM chef AS planner
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

# 阶段 3: 预编译依赖并构建可执行二进制
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# 预先编译所有第三方依赖（只要 Cargo.lock 没变，此步骤在构建时完全命中缓存）
RUN cargo chef cook --release --recipe-path recipe.json

# 拷贝源码并进行正式构建
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src

# 支持从构建参数注入 Git Commit 信息，供 status 端点与日志展示
ARG GIT_HASH
ARG GIT_COMMIT_DATE
ENV GIT_HASH=${GIT_HASH} \
    GIT_COMMIT_DATE=${GIT_COMMIT_DATE}

RUN cargo build --release --bin amll-ttml-api

# 阶段 4: 极简生产运行环境
FROM ubuntu:24.04 AS runner

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/amll-ttml-api /app/amll-ttml-api

RUN mkdir -p /data /var/lib/amll-ttml-api/analytics

ENV PORT=3000 \
    DATABASE_URL="sqlite:///data/amll_lyrics.db?mode=rwc"

EXPOSE 3000
VOLUME ["/data", "/var/lib/amll-ttml-api/analytics"]

ENTRYPOINT ["/app/amll-ttml-api"]
