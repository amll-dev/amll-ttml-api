//! 后台写入任务
//!
//! 中间件把记录投进有界 channel 后立即返回，这里按批写入缓冲库。统计数据丢一点无关紧要，
//! 但绝不能拖慢或搞挂正常请求，所以：
//! - channel 满了直接丢弃并计数，不阻塞请求
//! - 写入失败只记日志，这一批不重试
//! - 磁盘剩余空间低于水位时整批丢弃，不与主服务争空间
//!
//! 日志一律只在状态切换时打，避免持续故障时每秒一条错误把 Sentry 配额刷光

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{
            AtomicU64,
            Ordering,
        },
    },
    time::{
        Duration,
        Instant,
    },
};

use sea_orm::DatabaseConnection;
use tokio::{
    sync::{
        mpsc,
        oneshot,
    },
    task::JoinHandle,
    time::MissedTickBehavior,
};
use tracing::{
    error,
    info,
    warn,
};

use super::{
    buffer,
    record::RequestRecord,
};

/// channel 容量。按峰值约 100 请求/秒估算，写入任务卡住 100 秒才会开始丢数据
pub const CHANNEL_CAPACITY: usize = 10_000;
/// 单个事务最多写入的条数
const BATCH_MAX: usize = 2_000;
/// 攒批的最长时间，也是崩溃时最多丢失的数据窗口
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// 磁盘剩余空间的检查间隔，`statvfs` 虽便宜也没必要每批都查
const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// 两次「channel 已满」告警之间的最短间隔
const DROP_WARN_INTERVAL: Duration = Duration::from_hours(1);
/// 停机时等待写入任务刷盘的上限
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// 写入任务的运行参数
pub struct WriterOptions {
    pub db: DatabaseConnection,
    /// 缓冲库所在目录，用于检查剩余空间
    pub dir: PathBuf,
    /// 剩余空间低于此值时停止写入
    pub min_free_bytes: u64,
    /// 中间件因 channel 已满而丢弃的条数，由写入任务定期取走
    pub dropped: Arc<AtomicU64>,
}

/// 后台写入任务的控制柄，停机时用来刷盘
pub struct AnalyticsWriter {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl AnalyticsWriter {
    #[must_use]
    pub fn spawn(options: WriterOptions, rx: mpsc::Receiver<RequestRecord>) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let task = tokio::spawn(Writer::new(options).run(rx, stop_rx));
        Self { stop, task }
    }

    /// 通知写入任务把 channel 里剩下的记录写完后退出
    ///
    /// 应在 HTTP 服务优雅停机之后调用：此时在途请求都已结束，它们的记录都已进了 channel
    pub async fn shutdown(self) {
        let _ = self.stop.send(());
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, self.task).await {
            Ok(Ok(())) => info!("Request analytics writer flushed and stopped"),
            Ok(Err(e)) => error!("Request analytics writer task failed: {e:?}"),
            Err(_) => warn!("Timed out flushing request analytics on shutdown"),
        }
    }
}

struct Writer {
    options: WriterOptions,
    buf: Vec<RequestRecord>,
    low_disk: bool,
    last_disk_check: Option<Instant>,
    failing: bool,
    pending_drops: u64,
    last_drop_warn: Option<Instant>,
}

impl Writer {
    fn new(options: WriterOptions) -> Self {
        Self {
            options,
            buf: Vec::with_capacity(BATCH_MAX),
            low_disk: false,
            last_disk_check: None,
            failing: false,
            pending_drops: 0,
            last_drop_warn: None,
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<RequestRecord>, mut stop: oneshot::Receiver<()>) {
        let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            // 满批时当场刷盘，所以 limit 恒大于 0，recv_many 返回 0 只意味着所有发送端都已释放
            let limit = BATCH_MAX - self.buf.len();

            tokio::select! {
                received = rx.recv_many(&mut self.buf, limit) => {
                    if received == 0 {
                        break;
                    }
                    if self.buf.len() >= BATCH_MAX {
                        self.flush().await;
                    }
                }
                _ = ticker.tick() => {
                    self.flush().await;
                    self.report_drops();
                }
                // 控制柄被直接丢弃（例如启动失败提前退出）也走这里，同样先刷盘再退出
                _ = &mut stop => break,
            }
        }

        rx.close();
        while let Ok(record) = rx.try_recv() {
            self.buf.push(record);
            if self.buf.len() >= BATCH_MAX {
                self.flush().await;
            }
        }
        self.flush().await;
        self.report_drops();
    }

    async fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }

        if self.disk_low() {
            self.buf.clear();
            return;
        }

        match buffer::insert_batch(&self.options.db, &self.buf).await {
            Ok(()) => {
                if self.failing {
                    self.failing = false;
                    info!("Request analytics writes recovered");
                }
            }
            Err(e) => {
                if !self.failing {
                    self.failing = true;
                    error!(
                        records = self.buf.len(),
                        "Failed to write request analytics, dropping batches until writes recover: {e}"
                    );
                }
            }
        }

        self.buf.clear();
    }

    fn disk_low(&mut self) -> bool {
        let due = self
            .last_disk_check
            .is_none_or(|checked| checked.elapsed() >= DISK_CHECK_INTERVAL);
        if !due {
            return self.low_disk;
        }
        self.last_disk_check = Some(Instant::now());

        match fs4::available_space(&self.options.dir) {
            Ok(available) => {
                let low = available < self.options.min_free_bytes;
                if low && !self.low_disk {
                    error!(
                        available_bytes = available,
                        min_free_bytes = self.options.min_free_bytes,
                        "Free disk space below the analytics watermark, pausing request analytics"
                    );
                } else if !low && self.low_disk {
                    info!(
                        available_bytes = available,
                        "Free disk space recovered, resuming request analytics"
                    );
                }
                self.low_disk = low;
            }
            // 查不到剩余空间时沿用上一次的判断，不因为统计不到就停写
            Err(e) => warn!("Failed to query free disk space for request analytics: {e}"),
        }

        self.low_disk
    }

    fn report_drops(&mut self) {
        self.pending_drops += self.options.dropped.swap(0, Ordering::Relaxed);
        if self.pending_drops == 0 {
            return;
        }

        let due = self
            .last_drop_warn
            .is_none_or(|warned| warned.elapsed() >= DROP_WARN_INTERVAL);
        if due {
            warn!(
                dropped = self.pending_drops,
                "Request analytics channel was full, records dropped"
            );
            self.pending_drops = 0;
            self.last_drop_warn = Some(Instant::now());
        }
    }
}
