# AMLL TTML API

[AMLL TTML DataBase](https://github.com/amll-dev/amll-ttml-db) API 服务，提供 TTML 格式歌词的获取与搜索功能。

## API 接口

详细接口文档请参阅 <https://amll.dev/reference/http-api/overview>，
OpenAPI 规范见 <https://amll.dev/api/ttml/openapi.yaml>。

## 开发与运行

### 运行环境依赖

- [Rust](https://rustup.rs/)

### 本地开发运行

```bash
# 启动本地开发服务器（默认监听 0.0.0.0:3000）
cargo run

# 自定义启动端口
PORT=8080 cargo run
```

### 构建与部署

#### 1. 直接构建

```bash
# 构建 Release 版本
cargo build --release

# 运行二进制文件
./target/release/amll-ttml-api
```

#### 2. Docker 部署

##### 使用预构建镜像

直接使用从 GitHub Container Registry 发布的官方镜像：

```bash
mkdir -p data

docker run -d \
  --name amll-ttml-api \
  --restart unless-stopped \
  -p 3000:3000 \
  -v $(pwd)/data:/data \
  ghcr.io/amll-dev/amll-ttml-api:latest
```

或使用 `docker compose` 一键启动：

```bash
docker compose up -d
```

##### 本地构建镜像

```bash
# 构建 Docker 镜像
docker build -t amll-ttml-api .

# 运行 Docker 容器
docker run -d -p 3000:3000 -v $(pwd)/data:/data --name amll-ttml-api amll-ttml-api
```

## 许可

本项目采用 MIT 或 Apache-2.0 双许可证。

- [MIT License](./LICENSE-MIT)
- [Apache License 2.0](./LICENSE-APACHE)
