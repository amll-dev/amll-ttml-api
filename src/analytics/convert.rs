//! 每日转换：北京时间自然日收齐后，把缓冲库里这一天的行转成 Parquet，再从缓冲库删掉
//!
//! 每 5 分钟检查一次。过了 0 点 10 分才处理前一天，给还在写入任务缓冲里的数据留出宽限；
//! 缓冲里积压的多个整天（例如这个功能上线前积攒的）会在一轮里依次补转。
//!
//! 蓝绿两个实例都跑这个任务，靠 `conversions` 表的认领保证同一天只转换一次：
//! - 先写临时文件再 rename 到位，然后标记完成，最后按小批删除原始行
//! - 任何一步崩溃都能恢复：rename 之前崩溃，认领租约过期后重新转换并覆盖文件；
//!   标记完成之后崩溃，残留的原始行在下一轮被当作「已转换日期的残留」删掉
//!
//! 每轮转换之后执行保留策略

use std::{
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use chrono::NaiveDate;
use sea_orm::DatabaseConnection;
use tokio::{
    task::JoinHandle,
    time::MissedTickBehavior,
};
use tracing::{
    error,
    info,
    warn,
};

use super::{
    buffer::{
        self,
        Claim,
    },
    day::{
        DAY_MS,
        day_of,
        day_start_ms,
        file_name,
    },
    parquet_file::DailyFileWriter,
    record::now_millis,
    retention::{
        self,
        RetentionPolicy,
    },
};

/// 每日文件在统计目录下的子目录
pub const PARQUET_DIR: &str = "parquet";

const CONVERT_INTERVAL: Duration = Duration::from_mins(5);
/// 过了 0 点多久才转换前一天
const GRACE_MS: i64 = 10 * 60 * 1000;
/// 认领的租约。正常转换一天只要几秒，超过这个时长仍未完成视为持有者已经崩溃
const LEASE_MS: i64 = 30 * 60 * 1000;
/// 每次从缓冲库读取的行数，给内存封顶
const READ_CHUNK_ROWS: u64 = 10_000;

pub struct ConverterOptions {
    /// 转换任务专用的连接，长时间的读取不占写入任务的连接
    pub db: DatabaseConnection,
    /// 统计目录
    pub dir: PathBuf,
    pub instance: Arc<str>,
    pub retention: RetentionPolicy,
}

pub fn spawn(options: ConverterOptions) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CONVERT_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut failing = false;

        loop {
            ticker.tick().await;

            // 只在状态切换时打日志，持续故障时不至于每 5 分钟往 Sentry 报一条
            match run_once(&options, now_millis()).await {
                Ok(()) => {
                    if failing {
                        failing = false;
                        info!("Request analytics conversion recovered");
                    }
                }
                Err(e) => {
                    if !failing {
                        failing = true;
                        error!("Request analytics conversion failed, will retry: {e:?}");
                    }
                }
            }
        }
    })
}

/// 转换所有已收齐的整天，然后执行保留策略
pub async fn run_once(options: &ConverterOptions, now: i64) -> anyhow::Result<()> {
    convert_closed_days(options, now).await?;

    let today = day_of(now).context("Current time is out of range")?;
    let dir = options.dir.clone();
    let policy = options.retention;
    tokio::task::spawn_blocking(move || {
        retention::apply(&dir, &dir.join(PARQUET_DIR), today, &policy)
    })
    .await?
    .context("Failed to apply analytics retention")
}

async fn convert_closed_days(options: &ConverterOptions, now: i64) -> anyhow::Result<()> {
    // 截止点是「往前推宽限期后所在北京日」的 0 点，早于它的整天都已收齐
    let cutoff = day_of(now - GRACE_MS)
        .map(day_start_ms)
        .context("Current time is out of range")?;

    while let Some(oldest) = buffer::oldest_ts_before(&options.db, cutoff).await? {
        let day = day_of(oldest).context("Buffered timestamp is out of range")?;
        let day_key = day.format("%Y-%m-%d").to_string();
        let start = day_start_ms(day);

        let claim = buffer::claim_day(
            &options.db,
            &day_key,
            &options.instance,
            now,
            now - LEASE_MS,
        )
        .await?;

        match claim {
            Claim::Acquired { claimed_at } => {
                convert_day(options, day, &day_key, claimed_at).await?;
            }
            Claim::Done => {
                let deleted = buffer::delete_range(&options.db, start, start + DAY_MS).await?;
                warn!(
                    day = %day_key,
                    deleted,
                    "Deleted leftover buffered rows of an already converted day"
                );
            }
            // 另一个实例正在转换最旧的这一天，下一轮再看
            Claim::Held => break,
        }
    }

    Ok(())
}

async fn convert_day(
    options: &ConverterOptions,
    day: NaiveDate,
    day_key: &str,
    claimed_at: i64,
) -> anyhow::Result<()> {
    let start = day_start_ms(day);
    let end = start + DAY_MS;

    let parquet_dir = options.dir.join(PARQUET_DIR);
    tokio::fs::create_dir_all(&parquet_dir).await?;

    let mut writer =
        DailyFileWriter::create(&parquet_dir, &file_name(day), day_key, &options.instance)?;
    let mut after = None;

    loop {
        let rows = match buffer::read_range(&options.db, start, end, after, READ_CHUNK_ROWS).await {
            Ok(rows) => rows,
            Err(e) => {
                writer.abort();
                return Err(e.into());
            }
        };

        let Some(last) = rows.last() else {
            break;
        };
        after = Some((last.ts, last.row_id));
        writer = writer.write(rows).await?;
    }

    let summary = writer.finish().await?;

    let still_held = buffer::finish_day(
        &options.db,
        day_key,
        &options.instance,
        claimed_at,
        &summary,
        now_millis(),
    )
    .await?;
    if !still_held {
        // 租约过期后已被另一个实例接手，由它负责标记完成与删除原始行
        warn!(day = %day_key, "Lost the analytics conversion claim before finishing");
        return Ok(());
    }

    let deleted = buffer::delete_range(&options.db, start, end).await?;
    info!(
        day = %day_key,
        rows = summary.rows,
        bytes = summary.bytes,
        deleted,
        "Converted request analytics day to Parquet"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        path::Path,
    };

    use arrow_array::{
        Array,
        RecordBatch,
        StringArray,
        TimestampMillisecondArray,
    };
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use sea_orm::{
        ConnectionTrait,
        DatabaseBackend,
        Statement,
    };
    use sha2::{
        Digest,
        Sha256,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::analytics::{
        BUFFER_FILE,
        record::RequestRecord,
    };

    /// 北京时间 2026-09-27 0 点
    const SEP_27: i64 = 1_790_438_400_000;
    const HOUR: i64 = 3_600_000;

    async fn setup() -> (TempDir, ConverterOptions) {
        let dir = tempfile::tempdir().unwrap();
        let db = buffer::open(&dir.path().join(BUFFER_FILE)).await.unwrap();
        let options = ConverterOptions {
            db,
            dir: dir.path().to_owned(),
            instance: Arc::from("3000@test"),
            retention: RetentionPolicy {
                retention_days: 90,
                max_total_bytes: u64::MAX,
            },
        };
        (dir, options)
    }

    fn record(ts: i64, route: &str) -> RequestRecord {
        RequestRecord {
            ts,
            method: "GET".to_string(),
            prefix: Some("/v1"),
            route: Some(route.to_string()),
            raw_path: None,
            params: Some(r#"{"q":"x"}"#.to_string()),
            status: 200,
            latency_us: 1234,
            resp_bytes: Some(100),
            conditional: false,
            user_agent: Some("AMLL Player/1.0".to_string()),
            origin: None,
            referer: None,
            client_id: Some(-42),
            instance: Arc::from("3000@test"),
            hit_count: Some(1),
            hit_id: Some(7),
            match_kind: None,
            norm_query: None,
        }
    }

    async fn insert(options: &ConverterOptions, records: &[RequestRecord]) {
        buffer::insert_batch(&options.db, records).await.unwrap();
    }

    async fn buffered_count(options: &ConverterOptions) -> i64 {
        options
            .db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS n FROM requests",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }

    fn daily_path(dir: &TempDir, day: &str) -> std::path::PathBuf {
        dir.path().join(PARQUET_DIR).join(format!("{day}.parquet"))
    }

    fn read_parquet(path: &Path) -> (Vec<RecordBatch>, Option<String>) {
        let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path).unwrap()).unwrap();
        let schema_version = builder
            .metadata()
            .file_metadata()
            .key_value_metadata()
            .and_then(|kv| kv.iter().find(|kv| kv.key == "amll.schema_version"))
            .and_then(|kv| kv.value.clone());
        let batches = builder.build().unwrap().collect::<Result<_, _>>().unwrap();
        (batches, schema_version)
    }

    #[tokio::test]
    async fn converts_closed_days_and_keeps_today() {
        let (dir, options) = setup().await;
        insert(
            &options,
            &[
                // 09-26 的三行故意乱序写入，文件里应按 ts 排好
                record(SEP_27 - HOUR, "/lyrics/get"),
                record(SEP_27 - 20 * HOUR, "/lyrics/search"),
                record(SEP_27 - 1, "/lrclib/get"),
                // 09-25 一行
                record(SEP_27 - 30 * HOUR, "/status"),
                // 09-27（今天）一行
                record(SEP_27 + HOUR, "/lyrics/list"),
            ],
        )
        .await;

        run_once(&options, SEP_27 + 12 * HOUR).await.unwrap();

        assert!(daily_path(&dir, "2026-09-25").exists());
        assert!(!daily_path(&dir, "2026-09-27").exists(), "今天还没收齐");
        assert_eq!(buffered_count(&options).await, 1);

        let path = daily_path(&dir, "2026-09-26");
        let (batches, schema_version) = read_parquet(&path);
        assert_eq!(schema_version.as_deref(), Some("1"));
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 3);

        let batch = &batches[0];
        let ts = batch
            .column_by_name("ts")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .unwrap();
        assert_eq!(
            ts.values().to_vec(),
            [SEP_27 - 20 * HOUR, SEP_27 - HOUR, SEP_27 - 1]
        );
        let routes = batch
            .column_by_name("route")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(routes.value(0), "/lyrics/search");
        assert!(batch.column_by_name("match_kind").unwrap().is_null(0));

        // 转换记录里的 sha256 与字节数对得上文件本身
        let row = options
            .db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT state, rows, bytes, sha256, min_ts, max_ts FROM conversions WHERE day = '2026-09-26'",
            ))
            .await
            .unwrap()
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(row.try_get::<String>("", "state").unwrap(), "done");
        assert_eq!(row.try_get::<i64>("", "rows").unwrap(), 3);
        assert_eq!(
            row.try_get::<i64>("", "bytes").unwrap(),
            i64::try_from(bytes.len()).unwrap()
        );
        assert_eq!(
            row.try_get::<String>("", "sha256").unwrap(),
            hex::encode(Sha256::digest(&bytes))
        );
        assert_eq!(
            row.try_get::<i64>("", "min_ts").unwrap(),
            SEP_27 - 20 * HOUR
        );
        assert_eq!(row.try_get::<i64>("", "max_ts").unwrap(), SEP_27 - 1);

        // 再跑一轮是空操作
        run_once(&options, SEP_27 + 12 * HOUR).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(buffered_count(&options).await, 1);
    }

    #[tokio::test]
    async fn waits_for_the_grace_period_after_midnight() {
        let (dir, options) = setup().await;
        insert(&options, &[record(SEP_27 - HOUR, "/status")]).await;

        run_once(&options, SEP_27 + 5 * 60 * 1000).await.unwrap();
        assert!(!daily_path(&dir, "2026-09-26").exists());

        run_once(&options, SEP_27 + 11 * 60 * 1000).await.unwrap();
        assert!(daily_path(&dir, "2026-09-26").exists());
        assert_eq!(buffered_count(&options).await, 0);
    }

    #[tokio::test]
    async fn respects_a_live_claim_and_takes_over_an_expired_one() {
        let (dir, options) = setup().await;
        insert(&options, &[record(SEP_27 - HOUR, "/status")]).await;
        let now = SEP_27 + 12 * HOUR;

        let claimed = buffer::claim_day(&options.db, "2026-09-26", "3001@other", now - 60_000, 0)
            .await
            .unwrap();
        assert!(matches!(claimed, Claim::Acquired { .. }));

        run_once(&options, now).await.unwrap();
        assert!(
            !daily_path(&dir, "2026-09-26").exists(),
            "另一个实例的认领仍有效"
        );
        assert_eq!(buffered_count(&options).await, 1);

        run_once(&options, now + LEASE_MS).await.unwrap();
        assert!(daily_path(&dir, "2026-09-26").exists(), "租约过期后接手");
        assert_eq!(buffered_count(&options).await, 0);
    }

    #[tokio::test]
    async fn late_holder_cannot_finish_after_takeover() {
        let (_dir, options) = setup().await;
        let summary = buffer::DaySummary {
            rows: 0,
            bytes: 0,
            sha256: String::new(),
            min_ts: None,
            max_ts: None,
        };

        let Claim::Acquired { claimed_at: first } =
            buffer::claim_day(&options.db, "2026-09-26", "3000@a", 1_000, 0)
                .await
                .unwrap()
        else {
            panic!("first claim should succeed");
        };
        let Claim::Acquired { claimed_at: second } = buffer::claim_day(
            &options.db,
            "2026-09-26",
            "3001@b",
            1_000 + LEASE_MS + 1,
            1_001,
        )
        .await
        .unwrap() else {
            panic!("expired claim should be taken over");
        };

        assert!(
            !buffer::finish_day(&options.db, "2026-09-26", "3000@a", first, &summary, 0)
                .await
                .unwrap()
        );
        assert!(
            buffer::finish_day(&options.db, "2026-09-26", "3001@b", second, &summary, 0)
                .await
                .unwrap()
        );
        assert_eq!(
            buffer::claim_day(&options.db, "2026-09-26", "3000@a", i64::MAX, i64::MAX)
                .await
                .unwrap(),
            Claim::Done
        );
    }

    #[tokio::test]
    async fn deletes_leftover_rows_of_a_converted_day() {
        let (dir, options) = setup().await;
        insert(&options, &[record(SEP_27 - HOUR, "/status")]).await;
        run_once(&options, SEP_27 + 12 * HOUR).await.unwrap();
        let original = std::fs::read(daily_path(&dir, "2026-09-26")).unwrap();

        insert(&options, &[record(SEP_27 - 2 * HOUR, "/status")]).await;
        run_once(&options, SEP_27 + 12 * HOUR).await.unwrap();

        assert_eq!(buffered_count(&options).await, 0);
        assert_eq!(
            std::fs::read(daily_path(&dir, "2026-09-26")).unwrap(),
            original,
            "已发布的文件不再改动"
        );
    }

    #[tokio::test]
    async fn converts_a_multi_day_backlog_in_one_run() {
        let (dir, options) = setup().await;
        let records: Vec<_> = (1..=5)
            .map(|days_ago| record(SEP_27 - days_ago * DAY_MS + HOUR, "/status"))
            .collect();
        insert(&options, &records).await;

        run_once(&options, SEP_27 + 12 * HOUR).await.unwrap();

        for day in [
            "2026-09-22",
            "2026-09-23",
            "2026-09-24",
            "2026-09-25",
            "2026-09-26",
        ] {
            assert!(daily_path(&dir, day).exists(), "{day} 应已转换");
        }
        assert_eq!(buffered_count(&options).await, 0);
    }
}
