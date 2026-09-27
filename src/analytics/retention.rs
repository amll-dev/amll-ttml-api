//! 服务器上的保留策略：长期存档靠团队每月手动同步到网盘，服务器只保留一段滚动窗口
//!
//! 三道规则，按顺序执行：
//! 1. 早于保留天数的每日文件删除
//! 2. 统计目录总大小（缓冲库加全部每日文件）超过上限时，从最旧的每日文件开始删
//! 3. 崩溃遗留、超过一小时没动过的临时文件删除
//!
//! 蓝绿两个实例可能同时执行，同一个文件删两次时后者拿到 `NotFound`，忽略即可

use std::{
    fs,
    io,
    path::{
        Path,
        PathBuf,
    },
    time::{
        Duration,
        SystemTime,
    },
};

use chrono::NaiveDate;
use tracing::{
    info,
    warn,
};

use super::day::parse_file_name;

/// 临时文件多久没动过就视为崩溃遗留。正常转换一天的数据只要几秒
const STALE_TMP_AGE: Duration = Duration::from_hours(1);

#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    pub retention_days: u64,
    pub max_total_bytes: u64,
}

struct DailyFile {
    day: NaiveDate,
    path: PathBuf,
    bytes: u64,
}

/// 对统计目录执行保留策略，`parquet_dir` 是其下存放每日文件的子目录
pub fn apply(
    analytics_dir: &Path,
    parquet_dir: &Path,
    today: NaiveDate,
    policy: &RetentionPolicy,
) -> io::Result<()> {
    let mut daily = Vec::new();
    let mut other_bytes = top_level_bytes(analytics_dir)?;

    if parquet_dir.exists() {
        for entry in fs::read_dir(parquet_dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                continue;
            }

            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(day) = parse_file_name(&name) {
                daily.push(DailyFile {
                    day,
                    path: entry.path(),
                    bytes: metadata.len(),
                });
            } else if is_stale_tmp(&name, &metadata) {
                info!(file = %name, "Removing stale analytics temp file");
                remove(&entry.path())?;
            } else {
                other_bytes += metadata.len();
            }
        }
    }

    daily.sort_by_key(|file| file.day);

    let oldest_kept = today
        .checked_sub_days(chrono::Days::new(policy.retention_days))
        .unwrap_or(NaiveDate::MIN);
    let (expired, mut kept): (Vec<_>, Vec<_>) =
        daily.into_iter().partition(|file| file.day < oldest_kept);

    for file in expired {
        info!(day = %file.day, "Removing analytics file past the retention window");
        remove(&file.path)?;
    }

    let mut total = other_bytes + kept.iter().map(|file| file.bytes).sum::<u64>();
    while total > policy.max_total_bytes && !kept.is_empty() {
        let oldest = kept.remove(0);
        warn!(
            day = %oldest.day,
            total_bytes = total,
            max_total_bytes = policy.max_total_bytes,
            "Analytics directory over its size cap, removing the oldest daily file early"
        );
        remove(&oldest.path)?;
        total -= oldest.bytes;
    }

    Ok(())
}

/// 统计目录顶层文件（缓冲库及其 WAL / SHM）的总大小
fn top_level_bytes(dir: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(dir)? {
        let metadata = entry?.metadata()?;
        if metadata.is_file() {
            total += metadata.len();
        }
    }
    Ok(total)
}

fn is_stale_tmp(name: &str, metadata: &fs::Metadata) -> bool {
    let is_tmp = name.starts_with('.')
        && Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"));
    let age = metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
    is_tmp && age.is_some_and(|age| age >= STALE_TMP_AGE)
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::analytics::day::file_name;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn write_daily(dir: &Path, day: &str, bytes: usize) {
        fs::write(dir.join(file_name(date(day))), vec![0_u8; bytes]).unwrap();
    }

    fn remaining(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn removes_files_past_the_retention_window() {
        let root = tempfile::tempdir().unwrap();
        let parquet = root.path().join("parquet");
        fs::create_dir(&parquet).unwrap();
        for day in ["2026-06-28", "2026-06-29", "2026-09-26"] {
            write_daily(&parquet, day, 10);
        }

        let policy = RetentionPolicy {
            retention_days: 90,
            max_total_bytes: u64::MAX,
        };
        apply(root.path(), &parquet, date("2026-09-27"), &policy).unwrap();

        // 09-27 往前 90 天是 06-29，当天仍在窗口内
        assert_eq!(
            remaining(&parquet),
            ["2026-06-29.parquet", "2026-09-26.parquet"]
        );
    }

    #[test]
    fn size_cap_removes_oldest_first_and_counts_the_buffer() {
        let root = tempfile::tempdir().unwrap();
        let parquet = root.path().join("parquet");
        fs::create_dir(&parquet).unwrap();
        fs::write(root.path().join("buffer.db"), vec![0_u8; 50]).unwrap();
        for day in ["2026-09-24", "2026-09-25", "2026-09-26"] {
            write_daily(&parquet, day, 100);
        }

        // 50 + 3 × 100 = 350，上限 240：删一个剩 250 仍超，再删一个剩 150
        let policy = RetentionPolicy {
            retention_days: 90,
            max_total_bytes: 240,
        };
        apply(root.path(), &parquet, date("2026-09-27"), &policy).unwrap();

        assert_eq!(remaining(&parquet), ["2026-09-26.parquet"]);
        assert!(root.path().join("buffer.db").exists(), "缓冲库不参与删除");
    }

    #[test]
    fn keeps_fresh_temp_files_and_unknown_names() {
        let root = tempfile::tempdir().unwrap();
        let parquet = root.path().join("parquet");
        fs::create_dir(&parquet).unwrap();
        fs::write(parquet.join(".2026-09-26.parquet.3000-abc.tmp"), b"x").unwrap();
        fs::write(parquet.join("notes.txt"), b"x").unwrap();

        let policy = RetentionPolicy {
            retention_days: 90,
            max_total_bytes: u64::MAX,
        };
        apply(root.path(), &parquet, date("2026-09-27"), &policy).unwrap();

        // 刚写的临时文件可能正被另一个实例使用，不能删
        assert_eq!(
            remaining(&parquet),
            [".2026-09-26.parquet.3000-abc.tmp", "notes.txt"]
        );
    }
}
