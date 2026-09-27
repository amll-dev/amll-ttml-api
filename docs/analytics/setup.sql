-- 请求统计的 DuckDB 初始化脚本
--
-- 在仓库根目录启动 DuckDB，然后执行：
--     .read docs/analytics/setup.sql
-- 默认读取 ./amll-analytics/ 下的每日文件，即 scripts/analytics_sync.py 的默认下载目录；
-- 客户端归类表 clients.csv 也放在这个目录里（不入库，从团队网盘获取，见 docs/analytics.md）。
-- 文件放在别处时改下面两处路径。同步了新文件、或改了 clients.csv 之后重新执行一遍

-- 全部请求。union_by_name 让新旧列结构的文件可以混读（新增列在旧文件里读作 NULL）
-- ts 是 UTC 时刻；ts_local / day_local 是北京时间，与每日文件的切分口径一致
CREATE OR REPLACE VIEW requests AS
SELECT
    *,
    timezone('Asia/Shanghai', ts) AS ts_local,
    CAST(timezone('Asia/Shanghai', ts) AS DATE) AS day_local
FROM read_parquet('amll-analytics/*.parquet', union_by_name = true);

-- UA → 客户端的归类规则，按文件顺序匹配，第一条命中的生效
CREATE OR REPLACE TABLE client_rules AS
SELECT row_number() OVER () AS priority, pattern, client
FROM read_csv('amll-analytics/clients.csv', header = true, all_varchar = true);

-- 先对去重后的 UA 归类，再与请求关联，比逐行跑正则快得多
CREATE OR REPLACE TABLE ua_clients AS
SELECT
    ua.user_agent,
    coalesce(arg_min(r.client, r.priority), '其他') AS client
FROM (SELECT DISTINCT user_agent FROM requests WHERE user_agent IS NOT NULL) AS ua
LEFT JOIN client_rules AS r ON regexp_matches(ua.user_agent, r.pattern)
GROUP BY ua.user_agent;

-- 带客户端归类的请求
CREATE OR REPLACE VIEW requests_with_client AS
SELECT q.*, coalesce(c.client, '（无 UA）') AS client
FROM requests AS q
LEFT JOIN ua_clients AS c USING (user_agent);
