//! 数据同步器，负责把 GitHub 词库的最新数据同步到本地的数据库
//!
//! 触发源有三个：
//! 1. 进程启动
//! 2. 每 24 小时的定时任务
//! 3. webhook/sync API 调用
//!
//! 并发触发时只允许一个同步在途，其余直接跳过
//!
//! 每次同步依次执行：
//! 1. 从远端下载并写入 SQLite（`SyncService` 负责）
//! 2. 从 SQLite 重建内存索引
//! 3. 清空歌词缓存
//!
//! 索引是 SQLite 的派生视图，重建失败时保留旧索引继续服务，与库的落差最迟在下一次同步收敛。

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{
    error,
    info,
    warn,
};

use crate::{
    core::error::AppError,
    services::{
        db_lyric_store::DbLyricStore,
        sync_service::{
            SyncService,
            SyncStatus,
        },
    },
};

#[derive(Clone)]
pub struct LyricSyncer {
    store: DbLyricStore,
    lock: Arc<Mutex<()>>,
}

impl LyricSyncer {
    #[must_use]
    pub fn new(store: DbLyricStore) -> Self {
        Self {
            store,
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// 触发一次同步；已有同步在途时跳过（不排队）
    ///
    /// # Errors
    ///
    /// SQLite 同步失败时返回 [`AppError::UpstreamError`]；索引重建失败不报错，
    /// 保留旧索引继续服务
    pub async fn sync(&self) -> Result<(), AppError> {
        let Ok(_guard) = self.lock.try_lock() else {
            info!("Database sync is already in progress, skipping duplicate request.");
            return Ok(());
        };

        info!("Running database sync service...");
        let syncer = SyncService::new(self.store.conn.clone(), self.store.client.clone());
        match syncer.sync().await {
            Ok(res) => {
                info!("Sync completed with status: {:?}", res.status);
                if res.status == SyncStatus::Updated {
                    info!(
                        "Database was updated with new entries, rebuilding index and invalidating caches."
                    );
                    match self.store.rebuild_index().await {
                        Ok(()) => self.store.invalidate_caches(),
                        Err(e) => {
                            warn!("Index rebuild failed, keeping previous index: {e:?}");
                        }
                    }
                } else {
                    info!("Database was not changed, keeping existing in-memory index and caches.");
                }
                Ok(())
            }
            Err(e) => {
                error!("Sync failed: {e:?}");
                Err(AppError::UpstreamError(format!("{e:#}")))
            }
        }
    }
}
