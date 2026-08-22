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

# 全部测试。单测内联在 src 下的 #[cfg(test)] 模块
# （含 src/wire_format_tests.rs 线格式金样测试）；
# 集成测试 tests/architecture.rs 是依赖方向守卫
cargo test

# 只跑线格式金样测试（改动响应结构前后的快速验证）
cargo test wire_format

# 有意变更线格式时：重新生成快照，然后 review diff 再提交
INSTA_UPDATE=always cargo test wire_format

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

### 数据模型：SQLite 单一事实来源

这是理解本项目的关键。SQLite 是唯一事实来源，内存索引是它的派生缓存视图：

1. **SQLite**（`core/db/`）
   存完整数据：元数据（歌名、歌手、专辑、各平台 ID、作者，及 normalized 版本）、
   `raw_ttml` 原文与 `lyric_text` 正文。`lyrics` 主表 + `lyrics_fts`（FTS5 trigram 分词）
   虚表，靠三个 trigger 自动同步，歌词正文全文检索走这里。
   FTS5 建表失败只 warn 不 panic，因此正文检索属于可降级能力。
   表结构与查询同居一处：`setup.rs`（DDL）与 `queries.rs`（FTS 检索、按文件名取
   raw TTML、重建索引用的元数据投影查询）互为同一契约的两半，改任一侧必须同步另一侧。

2. **内存索引** `Arc<ArcSwap<LyricIndexDB>>`（`core/models.rs`、`core/repository.rs`）
   只含元数据的倒排索引（主键 ID / 平台 ID / 作者 → 条目），供 ID 精确查找与元数据
   模糊打分。不落盘：启动时与每次同步后从 SQLite 重建——`fetch_song_entries`
   （只投影元数据列，`raw_ttml` 等内容大列不进内存）→ `LyricIndexDB::from_entries`
   （`spawn_blocking` 组装）→ `swap_index`（`ArcSwap` 原子换入，读路径无锁）。

`LyricId`（`core/lyric_id.rs`）是歌词 ID 的强类型表示，封装 53 位 JavaScript Safe Integer
安全区间 (`0 ..= 0x001F_FFFF_FFFF_FFFF`)，提供边界校验、字符串解析以及由文件名生成哈希 ID 的统一接口。

### services 组件

- **`DbLyricStore`**（`services/db_lyric_store.rs`）：生产版 `LyricStore` 实现。
  三级取词（moka 缓存 → SQLite → GitHub 兜底 `github_fetcher`）、FTS 检索、
  索引持有与重建（`swap_index` / `invalidate_caches` / `rebuild_index`）。
  挂两层 moka 缓存：`ttml_cache`（原始 TTML）与 `formatted_lyric_cache`
  （LRC/纯文本解析结果），TTL 均为 168 小时。
- **`LyricSyncer`**（`services/lyric_syncer.rs`）：同步编排器，见下文同步服务。
- **`lyric_service`**（`services/lyric_service.rs`）：业务层自由函数，见下文请求分层。
- **`AppState`**（`services/app_state.rs`）：axum `State` 的全局状态，纯装配——
  持有 `store` / `syncer` / `start_time`（status 端点报 uptime）/ `sync_secret`
  （webhook 鉴权），自身不含业务方法。`Clone` 为浅拷贝，所有克隆共享同一套连接、缓存与锁。

### 请求分层

```
lib.rs 路由 → api/<模块>/extractor.rs → api/<模块>/handler.rs → services/lyric_service.rs → LyricStore trait（生产 DbLyricStore / 测试 MemoryLyricStore）
```

- `extractor.rs` 从 `RawQuery` 解析查询串，由 `api::shared::query`（`NATIVE_SEARCH_DIALECT` / `LRCLIB_SEARCH_DIALECT` / `LRCLIB_GET_DIALECT` 数据表）驱动，负责参数校验、别名映射、`q` 查询降级与分页提取。
  例如 `search` 里同时传 `q` 和具体字段时会丢弃 `q`；`get` 的优先级是 `id` > `filename` > 平台 ID 交集。
- `handler.rs` 保持极薄，只做「提参 → 调 service → 映射 DTO → 包装响应」。
  原生端点（`/lyrics/*`）经 `ApiSuccess<T>`（`api/shared/dto.rs`）自动附加
  `{"status": 200, "data": ...}` 信封，与错误侧 `AppError` 的 `IntoResponse` 对称
  （handler 签名统一为 `Result<ApiSuccess<T>, AppError>`）；lrclib 兼容端点返回
  裸数组/裸对象，不套信封。
- `lyric_service` 只返回领域数据（`SongEntry`、`LyricSearchResult`、元组配对），
  分页在 service 内完成；DTO 映射与线格式全部留在 api 层。
- `LyricStore` 统一抽象了 4 个 I/O 接口：`fetch_lyric_ttml`、`fetch_parsed_lyric`、`search_lyrics_fts` 和 `load_index`。
- 分页机制由 `core/pagination.rs` 的 `paginate` 组合器统一收转，提供安全步长切片与 `PaginationInfo` 分页元数据生成；FTS5 正文补全触发条件独立于 `page_size`，由业务常量 `FTS_TRIGGER_MIN_HITS` 专属判定。

### 检索评分与合并

`lyric_service::search_lyric` 把两路结果融合：

1. `LyricIndexDB::search_by_fields` 先粗筛（`rough_match`）再打分（`score_entry`），
   两者都在 `core/matcher/` 下（`rough.rs` / `score.rs`）：歌名/歌手/专辑加权（1.0 / 1.0 / 0.4），
   缺失维度会按比例放大分母，避免只传一个字段时分数被拉低。
   纯 `q` 走 `score_global_keyword`，思路是从关键词里先剥离歌手和专辑，剩余部分当歌名比对。
   单字段的比对细节在 `core/matcher/compare.rs`，档位枚举在 `types.rs`。
2. 仅当元数据结果偏弱（为空、首条低于 `Medium`、或数量不足 limit）或用户显式传了 `lyricText` 时，
   才追加 SQLite FTS5 正文检索。
3. 两路命中的合并与全序排序逻辑位于 `services/ranking.rs`（`merge_and_sort_hits`），按 5 级优先级桶（元数据强命中 > 元数据弱命中+正文 > 主歌词正文命中 > 仅元数据弱命中 > 仅背景人声命中 > 无命中）排序，
   同桶内再比 BM25 rank、元数据分、时间戳，并由 `entry.id` 提供保底严格全序。算法为纯函数设计，不依赖任何框架与异步环境，便于独立单元测试。

中日文匹配依赖 `core/matcher/normalize.rs` 里的 OpenCC 繁简转换（`convert_tw2s`），
入库时和查询时都会归一化，改动其中一侧必须同步另一侧，否则已有数据会失配。

### 分页机制

- **解析与校验**（`core/pagination.rs`）：由 `Pagination` 结构体处理，`page` 默认 1（从 1 起算），`pageSize` 默认 50，最大上限 100。传入 0、负数、非数字或 `pageSize` 超过 100 时返回 `400 Bad Request`；缺省或空字符串参数使用默认值。
- **响应数据结构**（`api/shared/dto.rs`）：`SearchData` 响应结果中 `items` 与分页参数解耦，分页元数据统一放在嵌套的 `pagination: PaginationInfo` 结构体中（字段包括 `page`, `pageSize`, `total`, `totalPages`, `hasMore`）。

### 同步服务

`services/sync_service.rs` 负责从远端下载并写 SQLite；`LyricSyncer` 负责编排
（single-flight 锁 + 索引重建 + 缓存失效）。触发源三处：启动时、每 24 小时定时、
`POST /v1/webhook/sync`（支持 `Authorization: Bearer SYNC_SECRET` 或 GitHub 原生
`X-Hub-Signature-256` HMAC-SHA256 签名校验）。

同步管线：先比对上游 `version.json` 的 commit，一致且本地非空则跳过（零网络）；
否则**无条件下载** `raw-lyrics-index.jsonl` 构建 `filename → 元数据` 映射——JSONL 是
平台 ID 等元数据的唯一来源（TTML 本体不含），下载失败即中止本次同步。与本地做差集后
按策略取内容：本地为空走全量（下载 `raw-lyrics.zip` 到临时文件，`spawn_blocking`
解压解析），本地非空走增量（并发 20 逐文件拉取）；增量待下载数超过 500 或失败时
回退全量。两条策略共用 `merge_raw_index_entry` 合并元数据后批量 upsert，
保证无论走哪条路径，落库的数据形状一致。

`LyricSyncer` 的锁（`Arc<Mutex<()>>`，所有克隆共享）用 `try_lock` 去重，
并发触发时后来者直接跳过而不是排队。同步成功（含跳过）后从 SQLite **本地**重建
内存索引（无网络）并清空两层缓存；重建失败只 warn 并保留旧索引，
与库的落差最迟下次同步收敛。

启动时 `main` 在 bind 端口前 await 一次本地索引重建：端口开放即搜索可用；
首次部署空库为空操作，随首次同步自愈。

### 其他约定

- **ID**：`core/lyric_id.rs` 用 FNV-1a 截断到 53 位，保证 JS `Number` 可安全表示。ID 由文件名派生，
  改哈希实现会让所有已发布 ID 失效。
- **错误**：类型在 core（`AppError`），HTTP 映射在 api（`api/shared/dto.rs` 的
  `IntoResponse` 实现，输出 `ErrorResponse` 形状的 `{status, error, message}`）。
  4xx 消息面向客户端（描述其输入问题）；5xx 的内部细节进日志 / Sentry
  （`IntoResponse` 里的 `error!`，sentry-tracing 的 ERROR 过滤器自动上报）。
  纯内部多步 IO 流水线（如 `sync_service`）允许 anyhow，跨组件边界必须转换，
  保留完整错误链。上游失败映射为 502。
- **build.rs**：注入 `GIT_HASH`、`GIT_COMMIT_DATE`、`RUSTC_VERSION`、`BUILD_TIME`，
  仅被 `/v1/status` 使用，改名字要同步 `api/status/handler.rs`。
- **接口变更**：改动响应结构或参数时，先跑 `cargo test wire_format` 确认本仓库侧的
  金样断言是否被破坏；接口文档和 OpenAPI 规范在 `applemusic-like-lyrics` 仓库
  （见开头链接），改完要去那边同步。

## 部署

GitHub Actions（`workflow_dispatch` 手动触发）交叉编译后 scp 到服务器，执行 `scripts/deploy-remote.sh`：
在 3000/3001 两个 systemd 实例间蓝绿切换，新端口通过 `/api/v1/status` 健康检查后才改 Nginx upstream
并停旧实例，检查失败则保留旧版本。环境变量由 `/etc/amll-ttml-api/env` 提供（见 `.env.example`）。
