//! 请求统计：逐请求记录，写入独立于歌词主库的 SQLite 热缓冲
//!
//! 数据流：[`middleware::record`] 采集 HTTP 层字段，handler 经 [`Annotator`] 补充命中数等业务标注
//! → 有界 channel → [`AnalyticsWriter`] 后台批量写入 `$ANALYTICS_DIR/buffer.db`
//!
//! 由环境变量 `ANALYTICS_DIR` 控制开关，未设置时中间件根本不挂载，本地开发与测试不受影响。
//! 统计出任何问题都不影响主服务：启动失败只关闭统计，运行期写入失败只丢数据

mod annotation;
mod buffer;
mod middleware;
mod record;
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
use hmac::KeyInit;
pub use middleware::record;
use record::{
    IpHasher,
    RequestRecord,
};
use tokio::sync::mpsc::{
    self,
    error::TrySendError,
};
use tracing::error;
pub use writer::AnalyticsWriter;
use writer::{
    CHANNEL_CAPACITY,
    WriterOptions,
};

/// 缓冲库在 `ANALYTICS_DIR` 下的文件名
const BUFFER_FILE: &str = "buffer.db";

/// 磁盘剩余空间低于 3 GiB 时停止写入，宁可丢统计数据，也不挤占主服务的空间
/// （全量同步要把 `raw-lyrics.zip` 下载到临时文件，主库还有 WAL）
pub const DEFAULT_MIN_FREE_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// 统计功能的配置
pub struct AnalyticsConfig {
    /// 缓冲库所在目录，不存在时自动创建
    pub dir: PathBuf,
    /// 对客户端 IP 做 HMAC 的密钥；缺失时照常记录，只是客户端标识留空
    pub ip_key: Option<Vec<u8>>,
    /// 实例标识，端口与 git hash
    pub instance: String,
    pub min_free_bytes: u64,
}

impl AnalyticsConfig {
    #[must_use]
    pub fn new(dir: PathBuf, ip_key: Option<Vec<u8>>, port: u16) -> Self {
        Self {
            dir,
            ip_key,
            instance: format!("{port}@{}", env!("GIT_HASH")),
            min_free_bytes: DEFAULT_MIN_FREE_BYTES,
        }
    }

    /// 从 `ANALYTICS_DIR` 与 `ANALYTICS_IP_KEY` 读取，`ANALYTICS_DIR` 未设置时返回 `None`
    #[must_use]
    pub fn from_env(port: u16) -> Option<Self> {
        let dir = non_empty_env("ANALYTICS_DIR")?;

        let ip_key = non_empty_env("ANALYTICS_IP_KEY");
        if ip_key.is_none() {
            error!(
                "ANALYTICS_IP_KEY is not set, request analytics will record no client identifiers"
            );
        }

        Some(Self::new(
            PathBuf::from(dir),
            ip_key.map(String::into_bytes),
            port,
        ))
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 统计功能的请求侧句柄，随 `AppState` 浅拷贝，所有克隆共享同一个 channel
#[derive(Clone)]
pub struct Analytics {
    tx: mpsc::Sender<RequestRecord>,
    ip_hasher: Option<Arc<IpHasher>>,
    instance: Arc<str>,
    dropped: Arc<AtomicU64>,
}

impl Analytics {
    /// 创建目录、打开缓冲库并启动后台写入任务
    ///
    /// 返回的 [`AnalyticsWriter`] 需要在 HTTP 服务停机后调用 `shutdown` 刷盘
    pub async fn start(config: AnalyticsConfig) -> anyhow::Result<(Self, AnalyticsWriter)> {
        tokio::fs::create_dir_all(&config.dir)
            .await
            .with_context(|| {
                format!(
                    "Failed to create analytics directory {}",
                    config.dir.display()
                )
            })?;

        let buffer_path = config.dir.join(BUFFER_FILE);
        let db = buffer::open(&buffer_path).await.with_context(|| {
            format!("Failed to open analytics buffer {}", buffer_path.display())
        })?;

        let ip_hasher = config
            .ip_key
            .map(|key| IpHasher::new_from_slice(&key).map(Arc::new))
            .transpose()
            .context("Invalid ANALYTICS_IP_KEY")?;

        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = AnalyticsWriter::spawn(
            WriterOptions {
                db,
                dir: config.dir,
                min_free_bytes: config.min_free_bytes,
                dropped: Arc::clone(&dropped),
            },
            rx,
        );

        let analytics = Self {
            tx,
            ip_hasher,
            instance: Arc::from(config.instance),
            dropped,
        };
        Ok((analytics, writer))
    }

    /// 非阻塞投递：channel 满了就丢弃并计数；写入任务已退出（停机阶段）时静默丢弃
    fn submit(&self, record: RequestRecord) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(record) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
