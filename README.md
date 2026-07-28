# AMLL TTML API

[AMLL TTML DataBase](https://github.com/amll-dev/amll-ttml-db) API 服务，提供 TTML 格式歌词的获取与搜索功能。

基于 [Axum](https://github.com/tokio-rs/axum) + [Tokio](https://tokio.rs/) + Rust 构建的高性能独立 HTTP API 服务。

## API 接口

| 接口                    | 方法 | 说明                                                                 |
| ----------------------- | ---- | -------------------------------------------------------------------- |
| `/api/v1/lyrics/get`    | GET  | 通过 `filename`、`id` 或平台 ID 获取单首歌曲的元数据及完整 TTML 歌词 |
| `/api/v1/lyrics/search` | GET  | 在词库中搜索符合条件的歌词（最多返回 50 条，不含歌词内容）           |
| `/api/v1/lrclib/get`    | GET  | LRCLIB 兼容接口 - 获取歌词                                           |
| `/api/v1/lrclib/search` | GET  | LRCLIB 兼容接口 - 搜索歌词                                           |

支持的定位方式：文件名、53 位 `id`、网易云音乐 / QQ 音乐 / Apple Music / Spotify / ISRC。

详细接口文档请参阅 [api.md](./api.md)。

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

```bash
# 构建 Docker 镜像
docker build -t amll-ttml-api .

# 运行 Docker 容器
docker run -d -p 3000:3000 --name amll-ttml-api amll-ttml-api
```

## 许可

本项目采用 MIT 或 Apache-2.0 双许可证。

- [MIT License](./LICENSE-MIT)
- [Apache License 2.0](./LICENSE-APACHE)
