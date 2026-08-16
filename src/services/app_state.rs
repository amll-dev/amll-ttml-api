//! 应用全局状态
//!
//! 进程内共享的一组句柄，在启动时构造，经 axum 的 `State` 提取器注入各 handler
//!
//! 持有四样东西：
//! 1. 歌词存储 [`DbLyricStore`]（取词、FTS 检索、内存索引）
//! 2. 数据同步器 [`LyricSyncer`]（从远端仓库刷新 SQLite 并重建索引）
//! 3. 启动时刻（status 端点报告正常运行时间用）
//! 4. webhook 鉴权密钥

use compact_str::CompactString;
use sea_orm::DatabaseConnection;

use crate::services::{
    db_lyric_store::DbLyricStore,
    lyric_syncer::LyricSyncer,
};

#[derive(Clone)]
pub struct AppState {
    pub store: DbLyricStore,
    pub syncer: LyricSyncer,
    pub start_time: std::time::Instant,
    pub sync_secret: Option<CompactString>,
}

impl AppState {
    /// 从环境变量 `SYNC_SECRET` 读取同步密钥构造
    pub fn new(db_conn: DatabaseConnection) -> Self {
        let sync_secret = std::env::var("SYNC_SECRET")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(CompactString::from);

        Self::new_with_secret(db_conn, sync_secret.as_deref())
    }

    /// 显式指定同步密钥构造（测试注入用）
    pub fn new_with_secret(db_conn: DatabaseConnection, secret: Option<&str>) -> Self {
        let store = DbLyricStore::new(db_conn);
        let syncer = LyricSyncer::new(store.clone());
        Self {
            store,
            syncer,
            start_time: std::time::Instant::now(),
            sync_secret: secret.map(CompactString::new),
        }
    }
}
