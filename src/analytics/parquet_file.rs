//! 每日 Parquet 文件的写入
//!
//! Arrow schema 与缓冲库 `requests` 表逐列对应。列一旦发布就是团队 SQL 手册依赖的契约：
//! 新增列时递增 [`SCHEMA_VERSION`]，读取方统一用 `read_parquet(..., union_by_name = true)`，
//! 新旧文件可以混读；改名或删列会破坏已归档的数据
//!
//! 服务器内存很紧（约 200 MiB 余量、无 swap），所以按小块流式写入，row group 同时受行数
//! 与字节数约束，内存占用封顶在几十 MiB

use std::{
    fs::{
        self,
        File,
    },
    io::{
        self,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
    sync::{
        Arc,
        LazyLock,
    },
};

use anyhow::Context;
use arrow_array::{
    ArrayRef,
    BooleanArray,
    Int32Array,
    Int64Array,
    RecordBatch,
    StringArray,
    TimestampMillisecondArray,
};
use arrow_schema::{
    DataType,
    Field,
    Schema,
    SchemaRef,
    TimeUnit,
};
use parquet::{
    arrow::ArrowWriter,
    basic::{
        Compression,
        ZstdLevel,
    },
    file::{
        metadata::KeyValue,
        properties::WriterProperties,
    },
};
use sha2::{
    Digest,
    Sha256,
};

use super::buffer::{
    BufferedRow,
    DaySummary,
};

/// 列结构版本，写进文件元数据 `amll.schema_version`
pub const SCHEMA_VERSION: &str = "1";

/// 每个 row group 的行数上限
const MAX_ROW_GROUP_ROWS: usize = 1_000_000;
/// 每个 row group 的编码后字节数上限，给内存封顶
const MAX_ROW_GROUP_BYTES: usize = 16 * 1024 * 1024;
const ZSTD_LEVEL: i32 = 3;

static SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    let text = |name: &str, nullable: bool| Field::new(name, DataType::Utf8, nullable);
    let int64 = |name: &str, nullable: bool| Field::new(name, DataType::Int64, nullable);

    Arc::new(Schema::new(vec![
        Field::new(
            "ts",
            DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
            false,
        ),
        text("method", false),
        text("prefix", true),
        text("route", true),
        text("raw_path", true),
        text("params", true),
        Field::new("status", DataType::Int32, false),
        int64("latency_us", false),
        int64("resp_bytes", true),
        Field::new("conditional", DataType::Boolean, false),
        text("user_agent", true),
        text("origin", true),
        text("referer", true),
        int64("client_id", true),
        text("instance", false),
        int64("hit_count", true),
        int64("hit_id", true),
        text("match_kind", true),
        text("norm_query", true),
    ]))
});

/// 边写边算 sha256 与字节数，省掉写完再读一遍文件
struct HashingWriter {
    file: File,
    hasher: Sha256,
    bytes: u64,
}

impl Write for HashingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.file.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// 一天的 Parquet 文件：先写到同目录的临时文件，完成后 rename 到位
///
/// 写入与压缩是 CPU 密集的同步操作，逐块放到 `spawn_blocking` 里做
pub struct DailyFileWriter {
    writer: ArrowWriter<HashingWriter>,
    tmp_path: PathBuf,
    final_path: PathBuf,
    rows: u64,
    min_ts: Option<i64>,
    max_ts: Option<i64>,
}

impl DailyFileWriter {
    /// 临时文件以 `.` 开头、`.tmp` 结尾，带上实例标识，两个实例同时写也不会互相覆盖
    pub fn create(dir: &Path, file_name: &str, day: &str, instance: &str) -> anyhow::Result<Self> {
        let tmp_path = dir.join(format!(".{file_name}.{}.tmp", instance.replace('@', "-")));
        let final_path = dir.join(file_name);

        let file = File::create(&tmp_path)
            .with_context(|| format!("Failed to create {}", tmp_path.display()))?;

        let properties = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(ZSTD_LEVEL)?))
            .set_max_row_group_row_count(Some(MAX_ROW_GROUP_ROWS))
            .set_max_row_group_bytes(Some(MAX_ROW_GROUP_BYTES))
            .set_key_value_metadata(Some(vec![
                KeyValue::new("amll.schema_version".to_owned(), SCHEMA_VERSION.to_owned()),
                KeyValue::new("amll.day".to_owned(), day.to_owned()),
                KeyValue::new("amll.timezone".to_owned(), "+08:00".to_owned()),
            ]))
            .build();

        let writer = ArrowWriter::try_new(
            HashingWriter {
                file,
                hasher: Sha256::new(),
                bytes: 0,
            },
            Arc::clone(&SCHEMA),
            Some(properties),
        )?;

        Ok(Self {
            writer,
            tmp_path,
            final_path,
            rows: 0,
            min_ts: None,
            max_ts: None,
        })
    }

    /// 追加一块按 `ts` 排好序的行
    pub async fn write(mut self, rows: Vec<BufferedRow>) -> anyhow::Result<Self> {
        if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
            self.min_ts = Some(self.min_ts.map_or(first.ts, |ts| ts.min(first.ts)));
            self.max_ts = Some(self.max_ts.map_or(last.ts, |ts| ts.max(last.ts)));
        }
        self.rows += rows.len() as u64;

        tokio::task::spawn_blocking(move || {
            let batch = to_record_batch(&rows)?;
            self.writer.write(&batch)?;
            Ok(self)
        })
        .await?
    }

    /// 写入文件尾、落盘并 rename 到位
    pub async fn finish(self) -> anyhow::Result<DaySummary> {
        tokio::task::spawn_blocking(move || {
            let HashingWriter {
                file,
                hasher,
                bytes,
            } = self.writer.into_inner()?;
            file.sync_all()?;
            drop(file);

            fs::rename(&self.tmp_path, &self.final_path).with_context(|| {
                format!(
                    "Failed to move Parquet file into {}",
                    self.final_path.display()
                )
            })?;

            Ok(DaySummary {
                rows: self.rows,
                bytes,
                sha256: hex::encode(hasher.finalize()),
                min_ts: self.min_ts,
                max_ts: self.max_ts,
            })
        })
        .await?
    }

    /// 放弃这次写入，删掉临时文件
    pub fn abort(self) {
        let tmp_path = self.tmp_path.clone();
        drop(self.writer);
        let _ = fs::remove_file(tmp_path);
    }
}

fn to_record_batch(rows: &[BufferedRow]) -> anyhow::Result<RecordBatch> {
    fn text<'a>(
        rows: &'a [BufferedRow],
        get: impl Fn(&'a BufferedRow) -> Option<&'a str>,
    ) -> ArrayRef {
        Arc::new(rows.iter().map(get).collect::<StringArray>())
    }
    fn int64(rows: &[BufferedRow], get: impl Fn(&BufferedRow) -> Option<i64>) -> ArrayRef {
        Arc::new(rows.iter().map(get).collect::<Int64Array>())
    }

    let columns: Vec<ArrayRef> = vec![
        Arc::new(
            TimestampMillisecondArray::from(rows.iter().map(|row| row.ts).collect::<Vec<_>>())
                .with_timezone("UTC"),
        ),
        text(rows, |row| Some(row.method.as_str())),
        text(rows, |row| row.prefix.as_deref()),
        text(rows, |row| row.route.as_deref()),
        text(rows, |row| row.raw_path.as_deref()),
        text(rows, |row| row.params.as_deref()),
        Arc::new(rows.iter().map(|row| row.status).collect::<Int32Array>()),
        int64(rows, |row| Some(row.latency_us)),
        int64(rows, |row| row.resp_bytes),
        Arc::new(
            rows.iter()
                .map(|row| Some(row.conditional))
                .collect::<BooleanArray>(),
        ),
        text(rows, |row| row.user_agent.as_deref()),
        text(rows, |row| row.origin.as_deref()),
        text(rows, |row| row.referer.as_deref()),
        int64(rows, |row| row.client_id),
        text(rows, |row| Some(row.instance.as_str())),
        int64(rows, |row| row.hit_count),
        int64(rows, |row| row.hit_id),
        text(rows, |row| row.match_kind.as_deref()),
        text(rows, |row| row.norm_query.as_deref()),
    ];

    Ok(RecordBatch::try_new(Arc::clone(&SCHEMA), columns)?)
}
