//! 请求统计的 SQLite 热缓冲
//!
//! 独立于歌词主库的文件。新请求先落在这里，崩溃最多丢掉写入任务里还没刷盘的约 1 秒数据；
//! 每个北京时间自然日收齐后由 [`super::convert`] 转成 Parquet，再从这里删掉。
//! 蓝绿切换时新旧两个实例会同时读写同一个文件，靠 WAL 与 `busy_timeout` 跨进程串行化写锁
//!
//! `requests` 的表结构、[`insert_batch`] 的列序与 [`BufferedRow`] 的字段互为同一契约，
//! 改任一处必须同步其余两处（以及 `parquet_file.rs` 的 Arrow schema）

use std::{
    path::Path,
    time::Duration,
};

use sea_orm::{
    ConnectOptions,
    ConnectionTrait,
    Database,
    DatabaseBackend,
    DatabaseConnection,
    DbErr,
    FromQueryResult,
    Statement,
    TransactionTrait,
    Value,
    sqlx::sqlite::{
        SqliteJournalMode,
        SqliteSynchronous,
    },
};

use super::{
    annotation::MatchKind,
    record::RequestRecord,
};

/// 缓冲库结构版本，写进 `PRAGMA user_version`，改表结构时递增
///
/// 1：`requests`；2：新增 `conversions`
const SCHEMA_VERSION: u32 = 2;

/// 另一个实例持有写锁时的最长等待。写入任务每秒只提交一次事务，
/// 转换任务的删除也按小批提交，正常竞争远到不了这个值
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 转换完成后按批删除原始行，每批的行数。一次删掉一整天会长时间占住写锁，
/// 挡住两个实例的写入任务
const DELETE_CHUNK_ROWS: u64 = 20_000;

const CREATE_REQUESTS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS requests (
    ts INTEGER NOT NULL,
    method TEXT NOT NULL,
    prefix TEXT,
    route TEXT,
    raw_path TEXT,
    params TEXT,
    status INTEGER NOT NULL,
    latency_us INTEGER NOT NULL,
    resp_bytes INTEGER,
    conditional INTEGER NOT NULL,
    user_agent TEXT,
    origin TEXT,
    referer TEXT,
    client_id INTEGER,
    instance TEXT NOT NULL,
    hit_count INTEGER,
    hit_id INTEGER,
    match_kind TEXT,
    norm_query TEXT
);
";

/// 按时间段导出和删除（每日转换）都走 `ts` 范围；索引项按 `(ts, rowid)` 排序，
/// 分块读取的 keyset 翻页也靠它
const CREATE_TS_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS requests_ts ON requests (ts);";

/// 每日转换的认领与结果
///
/// 蓝绿两个实例都会跑转换任务，同一天只能由一个实例转换：`state = 'running'` 的认领带租约，
/// 持有者崩溃后租约过期即可被接手。转换完成后记下文件的行数、字节数、sha256 与时间范围，
/// 下载端点的文件清单直接读这里，不必重新计算哈希
const CREATE_CONVERSIONS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS conversions (
    day TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    claimed_by TEXT NOT NULL,
    claimed_at INTEGER NOT NULL,
    rows INTEGER,
    bytes INTEGER,
    sha256 TEXT,
    min_ts INTEGER,
    max_ts INTEGER,
    finished_at INTEGER
);
";

const INSERT_SQL: &str = r"
INSERT INTO requests (
    ts, method, prefix, route, raw_path, params, status, latency_us, resp_bytes, conditional,
    user_agent, origin, referer, client_id, instance, hit_count, hit_id, match_kind, norm_query
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
";

/// 按 `(ts, rowid)` keyset 翻页读取某个时间段，顺序即写进 Parquet 的顺序
const READ_RANGE_SQL: &str = r"
SELECT
    rowid AS row_id, ts, method, prefix, route, raw_path, params, status, latency_us, resp_bytes,
    conditional, user_agent, origin, referer, client_id, instance, hit_count, hit_id, match_kind,
    norm_query
FROM requests
WHERE ts >= ? AND ts < ? AND (ts > ? OR (ts = ? AND rowid > ?))
ORDER BY ts, rowid
LIMIT ?;
";

/// 单条语句完成认领：没有记录时插入；已有记录只在仍是 `running` 且租约过期时接手。
/// 单条语句天然原子，两个实例同时认领时只有一个会改到行
const CLAIM_SQL: &str = r"
INSERT INTO conversions (day, state, claimed_by, claimed_at) VALUES (?, 'running', ?, ?)
ON CONFLICT (day) DO UPDATE SET claimed_by = excluded.claimed_by, claimed_at = excluded.claimed_at
WHERE conversions.state = 'running' AND conversions.claimed_at < ?;
";

/// 只有仍持有认领（实例与认领时刻都对得上）才能标记完成，租约被接手后的迟到者改不到行
const FINISH_SQL: &str = r"
UPDATE conversions
SET state = 'done', rows = ?, bytes = ?, sha256 = ?, min_ts = ?, max_ts = ?, finished_at = ?
WHERE day = ? AND state = 'running' AND claimed_by = ? AND claimed_at = ?;
";

const DELETE_RANGE_CHUNK_SQL: &str = r"
DELETE FROM requests WHERE rowid IN (
    SELECT rowid FROM requests WHERE ts >= ? AND ts < ? LIMIT ?
);
";

/// 缓冲库里的一行，供每日转换读取
#[derive(Debug, Clone, FromQueryResult)]
pub struct BufferedRow {
    pub row_id: i64,
    pub ts: i64,
    pub method: String,
    pub prefix: Option<String>,
    pub route: Option<String>,
    pub raw_path: Option<String>,
    pub params: Option<String>,
    pub status: i32,
    pub latency_us: i64,
    pub resp_bytes: Option<i64>,
    pub conditional: bool,
    pub user_agent: Option<String>,
    pub origin: Option<String>,
    pub referer: Option<String>,
    pub client_id: Option<i64>,
    pub instance: String,
    pub hit_count: Option<i64>,
    pub hit_id: Option<i64>,
    pub match_kind: Option<String>,
    pub norm_query: Option<String>,
}

/// 认领某一天的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// 认领成功，完成时凭认领时刻证明仍持有认领
    Acquired { claimed_at: i64 },
    /// 这一天已经转换完成
    Done,
    /// 另一个实例正在转换，租约未过期
    Held,
}

/// 一天转换完成后的结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaySummary {
    pub rows: u64,
    pub bytes: u64,
    pub sha256: String,
    pub min_ts: Option<i64>,
    pub max_ts: Option<i64>,
}

/// 一个已转换完成的每日文件，供下载端点的文件清单使用
#[derive(Debug, Clone, FromQueryResult)]
pub struct ConvertedDay {
    pub day: String,
    pub rows: i64,
    pub bytes: i64,
    pub sha256: String,
    pub min_ts: Option<i64>,
    pub max_ts: Option<i64>,
    pub finished_at: i64,
}

/// 打开（必要时创建）缓冲库并建表
pub async fn open(path: &Path) -> Result<DatabaseConnection, DbErr> {
    let path = path.to_owned();

    // URL 只用来让 sea-orm 选中 SQLite 后端，真正的文件路径在下面覆盖，
    // 避免 Windows 路径拼进 URL 时的转义问题
    let mut options = ConnectOptions::new("sqlite://analytics.db");
    options
        .max_connections(1)
        .sqlx_logging(false)
        .map_sqlx_sqlite_opts(move |opts| {
            opts.filename(&path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal)
                .busy_timeout(BUSY_TIMEOUT)
        });

    let db = Database::connect(options).await?;
    db.execute_unprepared(CREATE_REQUESTS_TABLE_SQL).await?;
    db.execute_unprepared(CREATE_TS_INDEX_SQL).await?;
    db.execute_unprepared(CREATE_CONVERSIONS_TABLE_SQL).await?;
    db.execute_unprepared(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
        .await?;

    Ok(db)
}

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, sql, values)
}

/// 在一个事务里写入一批记录
pub async fn insert_batch(db: &DatabaseConnection, records: &[RequestRecord]) -> Result<(), DbErr> {
    let txn = db.begin().await?;

    for record in records {
        let values = vec![
            record.ts.into(),
            record.method.clone().into(),
            record.prefix.into(),
            record.route.clone().into(),
            record.raw_path.clone().into(),
            record.params.clone().into(),
            i32::from(record.status).into(),
            record.latency_us.into(),
            record.resp_bytes.into(),
            record.conditional.into(),
            record.user_agent.clone().into(),
            record.origin.clone().into(),
            record.referer.clone().into(),
            record.client_id.into(),
            record.instance.as_ref().into(),
            record.hit_count.into(),
            record.hit_id.into(),
            record.match_kind.map(MatchKind::as_str).into(),
            record.norm_query.clone().into(),
        ];

        txn.execute_raw(statement(INSERT_SQL, values)).await?;
    }

    txn.commit().await
}

/// 早于 `cutoff` 的最早一条记录的时刻
pub async fn oldest_ts_before(db: &DatabaseConnection, cutoff: i64) -> Result<Option<i64>, DbErr> {
    let row = db
        .query_one_raw(statement(
            "SELECT MIN(ts) AS ts FROM requests WHERE ts < ?;",
            vec![cutoff.into()],
        ))
        .await?;

    row.map_or_else(|| Ok(None), |row| row.try_get::<Option<i64>>("", "ts"))
}

/// 读取 `[start, end)` 内排在 `after`（`(ts, rowid)`）之后的至多 `limit` 行，
/// 传 `None` 从头读起
pub async fn read_range(
    db: &DatabaseConnection,
    start: i64,
    end: i64,
    after: Option<(i64, i64)>,
    limit: u64,
) -> Result<Vec<BufferedRow>, DbErr> {
    let (after_ts, after_row) = after.unwrap_or((i64::MIN, i64::MIN));
    BufferedRow::find_by_statement(statement(
        READ_RANGE_SQL,
        vec![
            start.into(),
            end.into(),
            after_ts.into(),
            after_ts.into(),
            after_row.into(),
            limit.into(),
        ],
    ))
    .all(db)
    .await
}

/// 按小批删除 `[start, end)` 内的所有行，返回删除的行数
pub async fn delete_range(db: &DatabaseConnection, start: i64, end: i64) -> Result<u64, DbErr> {
    let mut deleted = 0;
    loop {
        let result = db
            .execute_raw(statement(
                DELETE_RANGE_CHUNK_SQL,
                vec![start.into(), end.into(), DELETE_CHUNK_ROWS.into()],
            ))
            .await?;

        let affected = result.rows_affected();
        deleted += affected;
        if affected < DELETE_CHUNK_ROWS {
            return Ok(deleted);
        }
    }
}

/// 认领某一天的转换，已有的 `running` 认领在 `lease_cutoff` 之前开始的视为过期
pub async fn claim_day(
    db: &DatabaseConnection,
    day: &str,
    instance: &str,
    now: i64,
    lease_cutoff: i64,
) -> Result<Claim, DbErr> {
    let result = db
        .execute_raw(statement(
            CLAIM_SQL,
            vec![day.into(), instance.into(), now.into(), lease_cutoff.into()],
        ))
        .await?;

    if result.rows_affected() == 1 {
        return Ok(Claim::Acquired { claimed_at: now });
    }

    let state = db
        .query_one_raw(statement(
            "SELECT state FROM conversions WHERE day = ?;",
            vec![day.into()],
        ))
        .await?
        .map(|row| row.try_get::<String>("", "state"))
        .transpose()?;

    Ok(match state.as_deref() {
        Some("done") => Claim::Done,
        _ => Claim::Held,
    })
}

/// 把认领标记为完成，返回是否仍持有认领
pub async fn finish_day(
    db: &DatabaseConnection,
    day: &str,
    instance: &str,
    claimed_at: i64,
    summary: &DaySummary,
    now: i64,
) -> Result<bool, DbErr> {
    let result = db
        .execute_raw(statement(
            FINISH_SQL,
            vec![
                i64::try_from(summary.rows).unwrap_or(i64::MAX).into(),
                i64::try_from(summary.bytes).unwrap_or(i64::MAX).into(),
                summary.sha256.clone().into(),
                summary.min_ts.into(),
                summary.max_ts.into(),
                now.into(),
                day.into(),
                instance.into(),
                claimed_at.into(),
            ],
        ))
        .await?;

    Ok(result.rows_affected() == 1)
}

/// 全部已转换完成的日期，按日期升序。文件可能已被保留策略删除，由调用方核对
pub async fn converted_days(db: &DatabaseConnection) -> Result<Vec<ConvertedDay>, DbErr> {
    ConvertedDay::find_by_statement(Statement::from_string(
        DatabaseBackend::Sqlite,
        r"
        SELECT day, rows, bytes, sha256, min_ts, max_ts, finished_at
        FROM conversions
        WHERE state = 'done'
        ORDER BY day;
        ",
    ))
    .all(db)
    .await
}
