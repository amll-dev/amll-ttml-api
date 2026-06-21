# AMLL TTML API

[AMLL TTML DataBase](https://github.com/amll-dev/amll-ttml-db) API 服务，提供 TTML 格式歌词的获取与搜索功能。

基于 [Cloudflare Workers](https://workers.cloudflare.com/) + Rust（[worker-rs](https://github.com/cloudflare/workers-rs)）构建。

## API 接口

| 接口          | 方法 | 说明                                                       |
| ------------- | ---- | ---------------------------------------------------------- |
| `/api/get`    | GET  | 通过平台 ID 获取单首歌曲的元数据及完整 TTML 歌词           |
| `/api/search` | GET  | 在词库中搜索符合条件的歌词（最多返回 50 条，不含歌词内容） |

支持的平台 ID：网易云音乐、QQ 音乐、Apple Music、Spotify、ISRC。

详细接口文档请参阅 [api.md](./api.md)。

## 开发

- [Rust](https://rustup.rs/)
- [wrangler CLI](https://developers.cloudflare.com/workers/wrangler/)（Cloudflare Workers 开发工具）
- `wasm32-unknown-unknown` 目标

```bash
# 安装 wasm 目标（如尚未安装）
rustup target add wasm32-unknown-unknown

# 安装 worker-build
cargo install worker-build
```

## 本地开发

```bash
# 启动本地开发服务器
wrangler dev
```

## 构建与部署

```bash
# 构建
worker-build --release

# 部署到 Cloudflare Workers
wrangler deploy
```

## 许可

本项目采用 MIT 或 Apache-2.0 双许可证。

- [MIT License](./LICENSE-MIT)
- [Apache License 2.0](./LICENSE-APACHE)
