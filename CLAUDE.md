# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

[AMLL TTML DataBase](https://github.com/amll-dev/amll-ttml-db) 的只读 API 服务，基于 Rust + Axum。
上游是 GitHub 仓库里的 TTML 歌词文件，本服务负责同步、索引、检索，并提供原生接口与 LRCLIB 兼容接口。

服务器部署与运维见 `deployment.md`。

接口文档在 AMLL 文档站，源文件在 `applemusic-like-lyrics` 仓库：

- 接口文档 <https://amll.dev/reference/http-api/overview>
  （源文件 `packages/docs/src/content/docs/reference/http-api/`，
  按 `overview` / `native` / `lrclib` / `system` 分成四个 `.mdx`）
- OpenAPI 规范 <https://amll.dev/api/ttml/openapi.yaml>
  （源文件 `packages/docs/public/api/ttml/openapi.yaml`）

## 常用命令

```bash
# 本地开发（默认 0.0.0.0:3000，可用 PORT 覆盖）
cargo run

# 全部测试（测试全部内联在 src 下的 #[cfg(test)] 模块，没有 tests/ 目录）
cargo test

# 单个测试 / 单个模块
cargo test find_by_id_exact_match
cargo test core::repository

# Lint。Cargo.toml 开启了 clippy pedantic + nursery，警告要清干净
cargo clippy --all-targets

# 格式化。rustfmt.toml 使用了 imports_granularity / group_imports 等 nightly 选项
cargo +nightly fmt

# Release 构建
cargo build --release
```

## 架构

### 双数据源

这是理解本项目的关键。同一批歌词同时存在于两处，用途不同：

1. **内存索引** `Arc<ArcSwap<LyricIndexDB>>`（`core/models.rs`、`core/repository.rs`）
   只含元数据（歌名、歌手、专辑、各平台 ID、作者），从上游 `raw-lyrics-index.jsonl` 构建。
   附带多个 `HashMap` 倒排索引，供 ID 精确查找和元数据模糊打分使用。
   整体替换而非增量更新，`ArcSwap` 保证读路径无锁。

2. **SQLite**（`core/db/setup.rs`）
   存完整数据，含 `raw_ttml` 与 `lyric_text`。`lyrics` 主表 + `lyrics_fts`（FTS5 trigram 分词）
   虚表，靠三个 trigger 自动同步。歌词正文全文检索走这里。
   FTS5 建表失败只 warn 不 panic，因此正文检索属于可降级能力。

`AppState`（`core/state.rs`）同时持有两者，并挂了两层 moka 缓存：`ttml_cache`（原始 TTML）和
`formatted_lyric_cache`（转成 LRC/纯文本的结果），TTL 均为 168 小时。

### 请求分层

```
lib.rs 路由 → api/<模块>/extractor.rs → api/<模块>/handler.rs → services/lyric_service.rs (LyricService<R: LyricStore>) → LyricStore Trait (AppState / MemoryLyricStore)
```

- `extractor.rs` 从 `RawQuery` 手工解析查询串（不是 axum 的 `Query` 提取器），负责参数校验与优先级。
  例如 `search` 里同时传 `q` 和具体字段时会丢弃 `q`；`get` 的优先级是 `id` > `filename` > 平台 ID 交集。
- `handler.rs` 保持极薄，只做「提参 → 调 service → 包 JSON」。
- `services/lyric_service.rs` 是检索编排的核心，定义了 `LyricService<R = AppState>`，面向深模块 trait `LyricStore`（`services/lyric_store.rs`）编程，解耦具体的 I/O 实现。
- `LyricStore` 统一抽象了 4 个 I/O 接口：`fetch_lyric_ttml`、`fetch_parsed_lyric`、`search_lyrics_fts` 和 `load_index`。生产环境由 `AppState` 实现，测试环境由 `MemoryLyricStore` 适配。

### 检索评分与合并

`LyricService::search_lyric` 把两路结果融合：

1. `LyricIndexDB::search_by_fields` 先粗筛（`rough_match`）再打分（`score_entry`），
   两者都在 `core/matcher/` 下（`rough.rs` / `score.rs`）：歌名/歌手/专辑加权（1.0 / 1.0 / 0.4），
   缺失维度会按比例放大分母，避免只传一个字段时分数被拉低。
   纯 `q` 走 `score_global_keyword`，思路是从关键词里先剥离歌手和专辑，剩余部分当歌名比对。
   单字段的比对细节在 `core/matcher/compare.rs`，档位枚举在 `types.rs`。
2. 仅当元数据结果偏弱（为空、首条低于 `Medium`、或数量不足 limit）或用户显式传了 `lyricText` 时，
   才追加 SQLite FTS5 正文检索。
3. `merge_and_sort_hits` 按优先级桶（元数据强命中 > 元数据+正文 > 主歌词命中 > …）排序，
   同桶内再比 bm25 rank、元数据分、时间戳。

中日文匹配依赖 `core/matcher/normalize.rs` 里的 OpenCC 繁简转换（`convert_tw2s`），
入库时和查询时都会归一化，改动其中一侧必须同步另一侧，否则已有数据会失配。

### 分页机制

- **解析与校验**（`api/shared/pagination.rs`）：由 `Pagination` 结构体处理，`page` 默认 1（从 1 起算），`pageSize` 默认 50，最大上限 100。传入 0、负数、非数字或 `pageSize` 超过 100 时返回 `400 Bad Request`；缺省或空字符串参数使用默认值。
- **响应数据结构**（`api/shared/dto.rs`）：`SearchData` 响应结果中 `items` 与分页参数解耦，分页元数据统一放在嵌套的 `pagination: PaginationInfo` 结构体中（字段包括 `page`, `pageSize`, `total`, `totalPages`, `hasMore`）。

### 同步服务

`services/sync_service.rs`，由三处触发：启动时、每 24 小时定时、`POST /v1/webhook/sync`（Bearer `SYNC_SECRET`）。

策略是先比对上游 `version.json` 的 commit：一致且本地非空则跳过；否则本地为空走全量
（下载 `raw-lyrics.zip` 到临时文件，`spawn_blocking` 解压解析），本地非空走增量
（对比 index 差集，并发 20 拉取）。增量待下载数超过 500 或失败时回退全量。
`AppState::sync_lock` 用 `try_lock` 做去重，并发触发时后来者直接返回而不是排队。

同步完成后重新拉取并整体替换内存索引。

### 其他约定

- **ID**：`utils/id.rs` 用 FNV-1a 截断到 53 位，保证 JS `Number` 可安全表示。ID 由文件名派生，
  改哈希实现会让所有已发布 ID 失效。
- **错误**：统一用 `core::error::AppError`，`IntoResponse` 输出 `{status, error, message}`。
  上游失败映射为 502。
- **build.rs**：注入 `GIT_HASH`、`GIT_COMMIT_DATE`、`RUSTC_VERSION`、`BUILD_TIME`，
  仅被 `/v1/status` 使用，改名字要同步 `api/status/handler.rs`。
- **接口变更**：改动响应结构或参数时，接口文档和 OpenAPI 规范都在 `applemusic-like-lyrics` 仓库（见开头链接），改完要去那边同步。

## 部署

GitHub Actions（`workflow_dispatch` 手动触发）交叉编译后 scp 到服务器，执行 `scripts/deploy-remote.sh`：
在 3000/3001 两个 systemd 实例间蓝绿切换，新端口通过 `/api/v1/status` 健康检查后才改 Nginx upstream
并停旧实例，检查失败则保留旧版本。环境变量由 `/etc/amll-ttml-api/env` 提供（见 `.env.example`）。
