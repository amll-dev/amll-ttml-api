use serde::Serialize;

use crate::analytics::DailyFile;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsFilesData {
    pub files: Vec<AnalyticsFileItem>,
}

/// 文件清单里的一项，同步脚本据此判断本地缺哪些文件、下载后校验完整性
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsFileItem {
    /// 文件名，也是下载路径的最后一段
    pub name: String,
    /// 北京时间日期
    pub day: String,
    pub rows: u64,
    pub bytes: u64,
    pub sha256: String,
    /// 文件内最早 / 最晚一条请求的时刻，Unix epoch 毫秒；空文件时省略
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_ts: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ts: Option<i64>,
    /// 转换完成的时刻，Unix epoch 毫秒
    pub converted_at: i64,
}

pub fn map_daily_file(file: DailyFile) -> AnalyticsFileItem {
    AnalyticsFileItem {
        name: file.name,
        day: file.day,
        rows: file.rows,
        bytes: file.bytes,
        sha256: file.sha256,
        first_ts: file.first_ts,
        last_ts: file.last_ts,
        converted_at: file.converted_at,
    }
}
