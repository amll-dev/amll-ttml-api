//! 请求统计：逐请求记录，先写入独立于歌词主库的 SQLite 热缓冲，每天转成一个 Parquet 文件
//!
//! 数据流：[`middleware::record`] 采集 HTTP 层字段，handler 经 [`Annotator`] 补充命中数等业务标注
//! → 有界 channel → 写入任务批量写入 `$ANALYTICS_DIR/buffer.db`
//! → 转换任务按北京时间自然日转成 `$ANALYTICS_DIR/parquet/YYYY-MM-DD.parquet`，并执行保留策略
//! → 团队经 `/v1/admin/analytics/files` 下载（`api/admin`）
//!
//! 由环境变量 `ANALYTICS_DIR` 控制开关，未设置时中间件根本不挂载，本地开发与测试不受影响。
//! 统计出任何问题都不影响主服务：启动失败只关闭统计，运行期写入失败只丢数据

mod annotation;
mod buffer;
mod convert;
mod day;
mod middleware;
mod parquet_file;
mod record;
mod retention;
mod writer;

#[cfg(test)]
mod tests;

use std::{
    env,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{
            AtomicU64,
            Ordering,
        },
    },
};

pub use annotation::{
    Annotator,
    MatchKind,
};
use anyhow::Context;
use chrono::NaiveDate;
use convert::ConverterOptions;
use hmac::KeyInit;
pub use middleware::record;
use record::{
    IpHasher,
    RequestRecord,
};
use retention::RetentionPolicy;
use sea_orm::{
    DatabaseConnection,
    DbErr,
};
use tokio::{
    sync::mpsc::{
        self,
        error::TrySendError,
    },
    task::JoinHandle,
};
use tracing::error;
use writer::{
    AnalyticsWriter,
    CHANNEL_CAPACITY,
    WriterOptions,
};

/// 缓冲库在 `ANALYTICS_DIR` 下的文件名
const BUFFER_FILE: &str = "buffer.db";

/// 磁盘剩余空间低于 3 GiB 时停止写入，宁可丢统计数据，也不挤占主服务的空间
/// （全量同步要把 `raw-lyrics.zip` 下载到临时文件，主库还有 WAL）
pub const DEFAULT_MIN_FREE_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// 每日文件在服务器上保留的天数。团队每月归档一次，留出约两个月的补救时间
pub const DEFAULT_RETENTION_DAYS: u64 = 90;

/// 统计目录（缓冲库加全部每日文件）的总大小上限
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// 统计功能的配置
pub struct AnalyticsConfig {
    /// 统计目录，不存在时自动创建
    pub dir: PathBuf,
    /// 对客户端 IP 做 HMAC 的密钥；缺失时照常记录，只是客户端标识留空
    pub ip_key: Option<Vec<u8>>,
    /// 下载端点的 Bearer 密钥；缺失时下载端点返回 500
    pub secret: Option<String>,
    /// 实例标识，端口与 git hash
    pub instance: String,
    pub min_free_bytes: u64,
    pub retention_days: u64,
    pub max_total_bytes: u64,
}

impl AnalyticsConfig {
    #[must_use]
    pub fn new(dir: PathBuf, ip_key: Option<Vec<u8>>, port: u16) -> Self {
        Self {
            dir,
            ip_key,
            secret: None,
            instance: format!("{port}@{}", env!("GIT_HASH")),
            min_free_bytes: DEFAULT_MIN_FREE_BYTES,
            retention_days: DEFAULT_RETENTION_DAYS,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }

    /// 从 `ANALYTICS_DIR`、`ANALYTICS_IP_KEY` 与 `ANALYTICS_SECRET` 读取，
    /// `ANALYTICS_DIR` 未设置时返回 `None`
    #[must_use]
    pub fn from_env(port: u16) -> Option<Self> {
        let dir = non_empty_env("ANALYTICS_DIR")?;

        let ip_key = non_empty_env("ANALYTICS_IP_KEY");
        if ip_key.is_none() {
            error!(
                "ANALYTICS_IP_KEY is not set, request analytics will record no client identifiers"
            );
        }

        Some(Self {
            secret: non_empty_env("ANALYTICS_SECRET"),
            ..Self::new(PathBuf::from(dir), ip_key.map(String::into_bytes), port)
        })
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 统计功能的句柄，随 `AppState` 浅拷贝，所有克隆共享同一个 channel 与连接
#[derive(Clone)]
pub struct Analytics {
    tx: mpsc::Sender<RequestRecord>,
    ip_hasher: Option<Arc<IpHasher>>,
    instance: Arc<str>,
    dropped: Arc<AtomicU64>,
    /// 下载端点用：每日文件目录、与转换任务共用的连接、鉴权密钥
    parquet_dir: Arc<PathBuf>,
    db: DatabaseConnection,
    secret: Option<Arc<str>>,
}

/// 一个可供下载的每日文件
#[derive(Debug, Clone)]
pub struct DailyFile {
    pub name: String,
    pub day: String,
    pub rows: u64,
    pub bytes: u64,
    pub sha256: String,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub converted_at: i64,
}

/// 后台任务的控制柄：写入任务与每日转换任务
pub struct AnalyticsTasks {
    writer: AnalyticsWriter,
    converter: JoinHandle<()>,
}

impl AnalyticsTasks {
    /// 停掉转换任务，再让写入任务把 channel 里剩下的记录写完
    ///
    /// 应在 HTTP 服务优雅停机之后调用：此时在途请求都已结束，它们的记录都已进了 channel。
    /// 转换中途被打断也无妨：临时文件由保留策略清理，认领租约过期后这一天会被重新转换
    pub async fn shutdown(self) {
        self.converter.abort();
        self.writer.shutdown().await;
    }
}

impl Analytics {
    /// 创建目录、打开缓冲库并启动后台任务
    ///
    /// 返回的 [`AnalyticsTasks`] 需要在 HTTP 服务停机后调用 `shutdown` 刷盘
    pub async fn start(config: AnalyticsConfig) -> anyhow::Result<(Self, AnalyticsTasks)> {
        tokio::fs::create_dir_all(&config.dir)
            .await
            .with_context(|| {
                format!(
                    "Failed to create analytics directory {}",
                    config.dir.display()
                )
            })?;

        // 写入任务与转换任务各用一个连接，转换时的长读取不挡写入
        let buffer_path = config.dir.join(BUFFER_FILE);
        let open = || async {
            buffer::open(&buffer_path).await.with_context(|| {
                format!("Failed to open analytics buffer {}", buffer_path.display())
            })
        };
        let writer_db = open().await?;
        let converter_db = open().await?;

        let ip_hasher = config
            .ip_key
            .map(|key| IpHasher::new_from_slice(&key).map(Arc::new))
            .transpose()
            .context("Invalid ANALYTICS_IP_KEY")?;
        let instance: Arc<str> = Arc::from(config.instance);

        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = AnalyticsWriter::spawn(
            WriterOptions {
                db: writer_db,
                dir: config.dir.clone(),
                min_free_bytes: config.min_free_bytes,
                dropped: Arc::clone(&dropped),
            },
            rx,
        );

        let parquet_dir = Arc::new(config.dir.join(convert::PARQUET_DIR));
        let converter = convert::spawn(ConverterOptions {
            db: converter_db.clone(),
            dir: config.dir,
            instance: Arc::clone(&instance),
            retention: RetentionPolicy {
                retention_days: config.retention_days,
                max_total_bytes: config.max_total_bytes,
            },
        });

        let analytics = Self {
            tx,
            ip_hasher,
            instance,
            dropped,
            parquet_dir,
            db: converter_db,
            secret: config.secret.map(Arc::from),
        };
        Ok((analytics, AnalyticsTasks { writer, converter }))
    }

    /// 非阻塞投递：channel 满了就丢弃并计数；写入任务已退出（停机阶段）时静默丢弃
    fn submit(&self, record: RequestRecord) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(record) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 下载端点的 Bearer 密钥
    #[must_use]
    pub fn secret(&self) -> Option<&str> {
        self.secret.as_deref()
    }

    /// 可供下载的每日文件，按日期升序；已被保留策略删掉的不列出
    pub async fn daily_files(&self) -> Result<Vec<DailyFile>, DbErr> {
        let mut files = Vec::new();

        for converted in buffer::converted_days(&self.db).await? {
            let Some(day) = NaiveDate::parse_from_str(&converted.day, "%Y-%m-%d").ok() else {
                continue;
            };
            let name = day::file_name(day);
            if !tokio::fs::try_exists(self.parquet_dir.join(&name))
                .await
                .unwrap_or(false)
            {
                continue;
            }

            files.push(DailyFile {
                name,
                day: converted.day,
                rows: converted.rows.cast_unsigned(),
                bytes: converted.bytes.cast_unsigned(),
                sha256: converted.sha256,
                first_ts: converted.min_ts,
                last_ts: converted.max_ts,
                converted_at: converted.finished_at,
            });
        }

        Ok(files)
    }

    /// 每日文件的路径，只接受严格符合 `YYYY-MM-DD.parquet` 的文件名，杜绝路径穿越
    #[must_use]
    pub fn daily_file_path(&self, name: &str) -> Option<PathBuf> {
        day::parse_file_name(name).map(|_| self.parquet_dir.join(name))
    }
}
