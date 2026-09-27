//! 每日文件的日界：北京时间（UTC+8，无夏令时，按固定偏移处理）
//!
//! 团队和用户主要在国内，「某一天」的问题正好对应一个文件。
//! `ts` 列本身始终是 UTC 毫秒，日界只决定一条请求落进哪个文件

use chrono::{
    DateTime,
    FixedOffset,
    NaiveDate,
    NaiveTime,
};

const BEIJING_OFFSET_SECS: i32 = 8 * 3600;
pub const DAY_MS: i64 = 86_400_000;
const FILE_SUFFIX: &str = ".parquet";

/// 该 UTC 毫秒时刻所在的北京时间日期
pub fn day_of(ts_ms: i64) -> Option<NaiveDate> {
    let beijing = FixedOffset::east_opt(BEIJING_OFFSET_SECS)?;
    DateTime::from_timestamp_millis(ts_ms).map(|t| t.with_timezone(&beijing).date_naive())
}

/// 北京时间某日 0 点对应的 UTC 毫秒
pub fn day_start_ms(day: NaiveDate) -> i64 {
    day.and_time(NaiveTime::MIN).and_utc().timestamp_millis()
        - i64::from(BEIJING_OFFSET_SECS) * 1000
}

/// 每日文件名，例如 `2026-09-26.parquet`
pub fn file_name(day: NaiveDate) -> String {
    format!("{}{FILE_SUFFIX}", day.format("%Y-%m-%d"))
}

/// [`file_name`] 的逆运算，只认严格符合格式的文件名
pub fn parse_file_name(name: &str) -> Option<NaiveDate> {
    let day = NaiveDate::parse_from_str(name.strip_suffix(FILE_SUFFIX)?, "%Y-%m-%d").ok()?;
    (file_name(day) == name).then_some(day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn day_boundary_is_beijing_midnight() {
        // 2026-09-26T15:59:59.999Z 是北京时间 09-26 23:59:59.999
        let last_ms_of_26 = 1_790_438_399_999;
        assert_eq!(day_of(last_ms_of_26), Some(date("2026-09-26")));
        assert_eq!(day_of(last_ms_of_26 + 1), Some(date("2026-09-27")));
        assert_eq!(day_start_ms(date("2026-09-27")), last_ms_of_26 + 1);
    }

    #[test]
    fn day_start_round_trips() {
        let day = date("2026-01-01");
        assert_eq!(day_of(day_start_ms(day)), Some(day));
        assert_eq!(day_of(day_start_ms(day) - 1), day.pred_opt());
        assert_eq!(
            day_start_ms(day) + DAY_MS,
            day_start_ms(day.succ_opt().unwrap())
        );
    }

    #[test]
    fn file_name_round_trips_strictly() {
        let day = date("2026-09-05");
        assert_eq!(file_name(day), "2026-09-05.parquet");
        assert_eq!(parse_file_name("2026-09-05.parquet"), Some(day));

        assert_eq!(parse_file_name("2026-9-5.parquet"), None);
        assert_eq!(parse_file_name("2026-09-05.parquet.tmp"), None);
        assert_eq!(parse_file_name("../2026-09-05.parquet"), None);
        assert_eq!(parse_file_name("buffer.db"), None);
    }
}
