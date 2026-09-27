# 请求统计

服务会记录每一个 HTTP 请求，每天整理成一个 Parquet 文件，供团队下载后用 DuckDB 分析。本文面向要看数据的团队成员；实现细节见 `CLAUDE.md` 的「请求统计」一节，服务器配置见 `deployment.md` 的 6.4 节。

## 数据是怎么来的

1. 每个请求结束时，记录它的路径、状态码、耗时、User-Agent 等字段，以及命中了哪首歌词等业务信息，先写进服务器上的 SQLite 缓冲。
2. 每天北京时间 0 点 10 分之后，前一天的数据转成一个文件 `YYYY-MM-DD.parquet`（按北京时间自然日切分），从缓冲里删掉。
3. 服务器只保留最近 **90 天**的文件（目录总量超过 8 GiB 时会提前删最旧的）。**长期存档靠团队每月手动同步到网盘**，见下文。

当天的数据要等到第二天 0 点 10 分之后才能下载；实时的错误监控仍然看 Sentry。

## 获取数据

### 1. 拿到 token

下载需要 `ANALYTICS_SECRET` 对应的 token，找维护者要。不要把 token 提交进仓库或贴到公开的地方；泄露了就让维护者在服务器上换一个。

### 2. 同步到本地

只需要 Python 3，不用装任何依赖。在仓库根目录运行：

```bash
AMLL_ANALYTICS_TOKEN=<token> python scripts/analytics_sync.py
```

Windows PowerShell：

```powershell
$env:AMLL_ANALYTICS_TOKEN = "<token>"; python scripts/analytics_sync.py
```

- 文件下载到 `./amll-analytics/`，可用 `--dest` 改位置。
- 只下载本地缺少的文件，重复运行是安全的；中断的下载下次会续传；下载完成后会校验 sha256。
- `--since 2026-09-01` 只同步某天之后的文件；`--verify` 对本地已有文件也重新校验。

### 3. 每月归档到网盘

服务器只留 90 天。**每月第一周**，由一名成员把上个月的文件同步下来并上传到团队网盘，按 `YYYY/YYYY-MM-DD.parquet` 存放。90 天的窗口留出了约两个月的补救时间，但忘了归档的文件过期后就找不回来了。

### 4. 不下载、直接远程查询（可选）

DuckDB 可以通过 HTTP Range 只读取用到的列，适合偶尔看一眼某一天：

```sql
INSTALL httpfs;
LOAD httpfs;
CREATE SECRET amll_analytics (
    TYPE http,
    EXTRA_HTTP_HEADERS MAP {'Authorization': 'Bearer <token>'}
);
SELECT route, count(*) AS requests
FROM read_parquet('https://api.amll.dev/v1/admin/analytics/files/2026-09-26.parquet')
GROUP BY ALL
ORDER BY requests DESC;
```

远程查询不支持通配符，一次要列出具体文件；跨多天分析还是先同步到本地。

## 用 DuckDB 分析

安装 [DuckDB](https://duckdb.org/docs/installation/)（CLI、Python 包都行；`duckdb -ui` 会打开一个网页界面，DBeaver 也能直接连）。在仓库根目录启动 DuckDB，先执行初始化脚本：

```sql
.read docs/analytics/setup.sql
```

它会建好这几个对象（同步了新文件后重新执行一遍）：

| 名称 | 说明 |
|---|---|
| `requests` | 全部请求，额外带 `ts_local`（北京时间）与 `day_local`（北京时间日期） |
| `requests_with_client` | 在 `requests` 基础上多一列 `client`：按客户端归类表 `amll-analytics/clients.csv` 归类出的客户端名 |
| `client_rules` / `ua_clients` | 归类规则与「UA → 客户端」映射，一般不用直接查 |

下面的查询都可以直接运行。

### A. 流量趋势

```sql
-- 每天的请求量与独立客户端数
SELECT day_local, count(*) AS requests, count(DISTINCT client_id) AS clients
FROM requests
GROUP BY ALL
ORDER BY day_local;
```

```sql
-- 各端点每小时的请求量（北京时间）
SELECT date_trunc('hour', ts_local) AS hour, route, count(*) AS requests
FROM requests
WHERE route IS NOT NULL
GROUP BY ALL
ORDER BY hour, requests DESC;
```

### B. 客户端分布

```sql
-- 各客户端的请求量、独立客户端数与占比
SELECT
    client,
    count(*) AS requests,
    count(DISTINCT client_id) AS clients,
    round(100 * count(*) / sum(count(*)) OVER (), 1) AS pct
FROM requests_with_client
GROUP BY ALL
ORDER BY requests DESC;
```

```sql
-- 还没归类的 UA，据此补充 clients.csv
SELECT user_agent, count(*) AS requests, count(DISTINCT client_id) AS clients
FROM requests_with_client
WHERE client = '其他'
GROUP BY ALL
ORDER BY requests DESC
LIMIT 50;
```

```sql
-- 浏览器里调用 API 的网站（Origin / Referer 只保留了 scheme + host）
SELECT coalesce(origin, referer) AS site, count(*) AS requests
FROM requests
WHERE coalesce(origin, referer) IS NOT NULL
GROUP BY ALL
ORDER BY requests DESC;
```

```sql
-- 各客户端走的是 /v1 还是 /api/v1，决定能否下线某个前缀时看这个
SELECT client, prefix, count(*) AS requests
FROM requests_with_client
WHERE prefix IS NOT NULL
GROUP BY ALL
ORDER BY client, requests DESC;
```

### C. 性能与错误

```sql
-- 各端点的延迟分位数与错误率
SELECT
    route,
    count(*) AS requests,
    round(quantile_cont(latency_us, 0.5) / 1000, 1) AS p50_ms,
    round(quantile_cont(latency_us, 0.95) / 1000, 1) AS p95_ms,
    round(quantile_cont(latency_us, 0.99) / 1000, 1) AS p99_ms,
    round(100 * avg((status >= 500)::INT), 2) AS pct_5xx,
    round(100 * avg((status BETWEEN 400 AND 499)::INT), 2) AS pct_4xx
FROM requests
WHERE route IS NOT NULL
GROUP BY ALL
ORDER BY requests DESC;
```

```sql
-- 没匹配到任何路由的请求：扫描器、写错路径的客户端
SELECT raw_path, count(*) AS requests
FROM requests
WHERE route IS NULL
GROUP BY ALL
ORDER BY requests DESC
LIMIT 20;
```

### D. 需求发现：大家在找、但词库里没有的歌

```sql
-- 按歌名 / 歌手查询却没找到的歌
-- norm_query 已做繁简转换与规范化，「周杰倫」和「周杰伦」算同一条
-- 按请求过的客户端数排序，避免被单个客户端反复重试刷上来
SELECT
    json_extract_string(norm_query, '$.track') AS track,
    json_extract_string(norm_query, '$.artist') AS artist,
    json_extract_string(norm_query, '$.q') AS keyword,
    count(*) AS requests,
    count(DISTINCT client_id) AS clients
FROM requests
WHERE hit_count = 0 AND norm_query IS NOT NULL
GROUP BY ALL
ORDER BY clients DESC, requests DESC
LIMIT 100;
```

```sql
-- 按网易云 ID 查询却没找到的歌，拿 ID 去网易云就能查到是哪首
-- 其他平台把 ncmMusicId 换成 qqMusicId / appleMusicId / spotifyId / isrc
SELECT
    json_extract_string(params, '$.ncmMusicId') AS ncm_music_id,
    count(*) AS requests,
    count(DISTINCT client_id) AS clients
FROM requests
WHERE route = '/lyrics/get'
    AND hit_count = 0
    AND json_extract_string(params, '$.ncmMusicId') IS NOT NULL
GROUP BY ALL
ORDER BY clients DESC, requests DESC
LIMIT 100;
```

### E. 热门歌词

```sql
-- 被取得最多的歌词（只算取词端点，搜索结果不算）
-- 歌名等信息用 https://api.amll.dev/v1/lyrics/get?id=<hit_id> 查
SELECT hit_id, count(*) AS requests, count(DISTINCT client_id) AS clients
FROM requests
WHERE hit_id IS NOT NULL AND route IN ('/lyrics/get', '/lrclib/get', '/lrclib/get/{id}')
GROUP BY ALL
ORDER BY requests DESC
LIMIT 50;
```

```sql
-- 客户端用什么方式查找歌词：ID、平台 ID、模糊匹配还是正文检索
SELECT route, match_kind, count(*) AS requests
FROM requests
WHERE match_kind IS NOT NULL
GROUP BY ALL
ORDER BY route, requests DESC;
```

### F. 独立客户端与滥用排查

```sql
-- 请求最多的客户端。client_id 是客户端 IP 的 HMAC 摘要，同一 IP 跨天不变，但无法反推 IP；
-- 真要封禁时，把 client_id、UA 与时间段交给有服务器权限的维护者，去 Nginx 日志里对原始 IP
SELECT
    client_id,
    count(*) AS requests,
    count(DISTINCT day_local) AS active_days,
    any_value(user_agent) AS sample_user_agent,
    min(ts_local) AS first_seen,
    max(ts_local) AS last_seen
FROM requests
WHERE client_id IS NOT NULL
GROUP BY client_id
ORDER BY requests DESC
LIMIT 20;
```

### G. 缓存效果

```sql
-- 带 If-None-Match 的条件请求占比，以及其中命中 304 的比例
SELECT
    route,
    count(*) AS requests,
    round(100 * avg(conditional::INT), 1) AS pct_conditional,
    round(100 * count_if(status = 304) / nullif(count_if(conditional), 0), 1) AS pct_304_of_conditional,
    round(sum(resp_bytes) / 1e6, 1) AS mb_sent
FROM requests
WHERE route IS NOT NULL
GROUP BY ALL
ORDER BY requests DESC;
```

## 字段说明

| 列 | 类型 | 说明 |
|---|---|---|
| `ts` | 时间戳（UTC） | 请求到达的时刻，毫秒精度 |
| `method` | 文本 | HTTP 方法；CORS 预检是 `OPTIONS` |
| `prefix` | 文本 | `/v1` 或 `/api/v1`；路径不带这两个前缀时为空 |
| `route` | 文本 | 去掉前缀的路由模板，例如 `/lrclib/get/{id}`；没匹配到路由时为空 |
| `raw_path` | 文本 | 原始路径，只在没匹配到路由时记录 |
| `params` | 文本（JSON） | 解码后的查询参数；同名参数重复出现时值是数组。最多 16 对，每个值最多 256 字节 |
| `status` | 整数 | HTTP 状态码 |
| `latency_us` | 整数 | 服务端耗时（微秒），到响应头生成为止；文件下载这类流式响应不含传输时间 |
| `resp_bytes` | 整数 | 响应体字节数（压缩前） |
| `conditional` | 布尔 | 请求是否带了 `If-None-Match` |
| `user_agent` | 文本 | 原样的 User-Agent，最多 512 字节 |
| `origin` | 文本 | `Origin` 请求头 |
| `referer` | 文本 | `Referer` 只保留 scheme + host |
| `client_id` | 整数 | 客户端 IP 的 HMAC 摘要（64 位）；本机请求（如健康检查）为空 |
| `instance` | 文本 | 处理请求的实例：`端口@git hash` |
| `hit_count` | 整数 | 命中数：搜索与列表是分页前的总数，取词是 0 或 1；其他端点为空 |
| `hit_id` | 整数 | 命中的歌词 ID：取词是命中的那首，搜索与列表是本页第一条 |
| `match_kind` | 文本 | `id` / `filename` / `platform_id` / `fuzzy`（元数据模糊匹配）/ `fts`（歌词正文检索）。取词端点记录查找方式（没找到也记），搜索端点记录第一条结果的来源 |
| `norm_query` | 文本（JSON） | 只在没找到时记录：规范化后的 `q` / `track` / `artist` / `album` |

文件元数据里的 `amll.schema_version` 是列结构版本，以后只会新增列，不会改名或删除。

## 维护客户端归类表

归类表**不放进仓库**：仓库是公开的，而这张表会暴露有哪些应用在用这个接口。它和每日文件一起存在团队网盘上，与归档放在同一个目录；分析前把它复制到 `amll-analytics/clients.csv`（这个目录已被 `.gitignore` 忽略）。`setup.sql` 找不到它会报错。

表有两列：`pattern` 是正则表达式（RE2 语法），`client` 是显示名。**按文件顺序匹配，第一条命中的生效**，所以具体的规则放前面，`浏览器`、`爬虫` 这类宽泛的放最后。例如：

```csv
pattern,client
^amll-analytics-sync/,amll-analytics-sync（本仓库同步脚本）
^SomePlayer/,SomePlayer
(?i)(bot|crawler|spider)\b,爬虫
^Mozilla/5\.0,浏览器
```

用上面 B 节「还没归类的 UA」查询找出新客户端，补进表里后把新版本传回网盘。

## 已知限制

- Nginx 限流直接返回的 429 到不了应用，不在统计里（大约占请求的 0.5%）。
- 统计优先保证不影响正常服务：写入队列满、磁盘剩余不足 3 GiB、或者进程崩溃时，会丢掉一部分记录（崩溃最多丢约 1 秒）。
- `norm_query` 只在没找到时记录；按 ID 查找的需求直接看 `params`。
- `client_id` 无法区分同一出口 IP 后面的多个用户（例如同一个校园网），也会把换了 IP 的同一用户算成两个。

## 接口参考

两个接口都要求 `Authorization: Bearer <token>`，响应一律不缓存。它们是内部接口，不在公开接口文档里。

- `GET /v1/admin/analytics/files`：文件清单。

  ```json
  {
    "status": 200,
    "data": {
      "files": [
        {
          "name": "2026-09-26.parquet",
          "day": "2026-09-26",
          "rows": 321661,
          "bytes": 7987654,
          "sha256": "…",
          "firstTs": 1790352000123,
          "lastTs": 1790438399876,
          "convertedAt": 1790439000456
        }
      ]
    }
  }
  ```

- `GET /v1/admin/analytics/files/{name}`：下载一个文件，支持 `Range`。
