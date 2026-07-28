use std::{
    collections::HashSet,
    io::{
        Read,
        Seek,
        SeekFrom,
        Write,
    },
    path::Path,
};

use anyhow::{
    Context,
    Result,
};
use futures::{
    StreamExt,
    stream,
};
use reqwest::Client;
use sea_orm::{
    DatabaseConnection,
    EntityTrait,
    PaginatorTrait,
    QuerySelect,
    Set,
};
use serde::{
    Deserialize,
    Serialize,
};
use tempfile::NamedTempFile;
use tracing::{
    info,
    warn,
};
use zip::read::ZipArchive;

use crate::{
    core::{
        db::entity::{
            self,
            meta,
        },
        models::RawIndexEntry,
    },
    utils::{
        id::generate_file_id,
        matcher::normalize_name_for_comparison,
    },
};

const DB_BASE: &str = "https://raw.githubusercontent.com/amll-dev/amll-ttml-db/main";
const INCREMENTAL_THRESHOLD: usize = 500;
const CONCURRENT_WORKERS: usize = 20;
const UPSERT_BATCH_SIZE: usize = 500;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RemoteVersion {
    pub build_date: Option<String>,
    pub commit: String,
    pub file_count: Option<usize>,
    pub timestamp: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    Skipped,
    Updated,
    Empty,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SyncResult {
    pub status: SyncStatus,
    pub count: Option<usize>,
    pub error: Option<String>,
    pub strategy: Option<String>,
}

pub struct SyncService {
    db: DatabaseConnection,
    client: Client,
}

impl SyncService {
    pub const fn new(db: DatabaseConnection, client: Client) -> Self {
        Self { db, client }
    }

    pub async fn sync(&self) -> Result<SyncResult> {
        let remote_version = match self.fetch_remote_version().await {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("Failed to fetch remote version: {e}, proceeding with sync check");
                None
            }
        };

        let local_commit = self.get_local_commit().await?;

        if let Some(ref remote) = remote_version {
            info!(
                "Local commit: {:?}, Remote commit: {}",
                local_commit.as_deref().unwrap_or("None"),
                &remote.commit[..7.min(remote.commit.len())]
            );

            if local_commit.as_deref() == Some(remote.commit.as_str()) {
                let local_count = self.get_local_count().await?;
                if local_count > 0 {
                    info!(
                        "Database is up to date with commit {}, skipping.",
                        remote.commit
                    );
                    return Ok(SyncResult {
                        status: SyncStatus::Skipped,
                        count: Some(local_count),
                        error: None,
                        strategy: None,
                    });
                }
            }
        }

        let local_files = self.get_all_local_filenames().await?;
        info!("Local DB contains {} entries", local_files.len());

        let result = if local_files.is_empty() {
            info!("No local data, performing full sync...");
            self.perform_full_sync(&local_files).await?
        } else {
            info!("Attempting incremental sync...");
            match self.perform_incremental_sync(&local_files).await {
                Ok(res) => res,
                Err(e) => {
                    warn!("Incremental sync failed: {e:?}, falling back to full sync");
                    self.perform_full_sync(&local_files).await?
                }
            }
        };

        if result.status != SyncStatus::Failed
            && let Some(remote) = remote_version
            && let Err(e) = self.save_local_commit(&remote.commit).await
        {
            warn!("Failed to save commit to local: {e}");
        }

        Ok(result)
    }

    async fn fetch_remote_version(&self) -> Result<RemoteVersion> {
        let url = format!("{DB_BASE}/raw-lyrics/version.json");
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Version endpoint returned HTTP {}", resp.status());
        }
        let v = resp.json::<RemoteVersion>().await?;
        Ok(v)
    }

    async fn get_local_commit(&self) -> Result<Option<String>> {
        let res = meta::Entity::find_by_id("commit".to_string())
            .one(&self.db)
            .await?;
        Ok(res.map(|m| m.value))
    }

    async fn save_local_commit(&self, commit: &str) -> Result<()> {
        let active = meta::ActiveModel {
            key: Set("commit".to_string()),
            value: Set(commit.to_string()),
        };
        meta::Entity::insert(active)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(meta::Column::Key)
                    .update_column(meta::Column::Value)
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;
        Ok(())
    }

    async fn get_local_count(&self) -> Result<usize> {
        let count = entity::Entity::find().count(&self.db).await?;
        Ok(count as usize)
    }

    async fn get_all_local_filenames(&self) -> Result<HashSet<String>> {
        let filenames: Vec<String> = entity::Entity::find()
            .select_only()
            .column(entity::Column::Filename)
            .into_tuple::<String>()
            .all(&self.db)
            .await?;
        Ok(filenames.into_iter().collect())
    }

    async fn perform_full_sync(&self, local_files: &HashSet<String>) -> Result<SyncResult> {
        let url = format!("{DB_BASE}/raw-lyrics/raw-lyrics.zip");
        info!("Downloading full lyrics archive from: {url}");

        let mut resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to download zip")?;
        if !resp.status().is_success() {
            anyhow::bail!("HTTP error downloading zip: {}", resp.status());
        }

        let mut temp_file = NamedTempFile::new().context("Failed to create temp file")?;
        let mut downloaded_bytes = 0;

        while let Some(chunk) = resp.chunk().await.context("Failed to read chunk")? {
            temp_file
                .write_all(&chunk)
                .context("Failed to write chunk")?;
            downloaded_bytes += chunk.len();
        }

        info!("Downloaded {downloaded_bytes} bytes, parsing zip archive...");
        temp_file.seek(SeekFrom::Start(0))?;

        let parsed_entries = tokio::task::spawn_blocking(move || -> Result<Vec<entity::Model>> {
            let mut archive =
                ZipArchive::new(temp_file.as_file()).context("Failed to open zip archive")?;
            let mut models = Vec::new();

            for i in 0..archive.len() {
                let mut file = archive.by_index(i).context("Failed to read zip entry")?;
                if Path::new(file.name())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("ttml"))
                    && !file.is_dir()
                {
                    let mut content = String::new();
                    if file.read_to_string(&mut content).is_ok()
                        && let Ok(result) = ttml_processor::parse_ttml(&content)
                    {
                        let filename = file.name().to_string();
                        let model = build_entity_from_ttml(&filename, &content, &result);
                        models.push(model);
                    }
                }
            }
            Ok(models)
        })
        .await??;

        let total_parsed = parsed_entries.len();
        let new_entries = filter_new_entries(parsed_entries, local_files);

        info!(
            "Parsed {} entries from zip archive. Found {} new entries to insert into SQLite...",
            total_parsed,
            new_entries.len()
        );

        if new_entries.is_empty() {
            return Ok(SyncResult {
                status: SyncStatus::Skipped,
                count: Some(0),
                error: None,
                strategy: Some("full".to_string()),
            });
        }

        self.batch_upsert_entries(&new_entries).await?;

        Ok(SyncResult {
            status: SyncStatus::Updated,
            count: Some(new_entries.len()),
            error: None,
            strategy: Some("full".to_string()),
        })
    }

    async fn perform_incremental_sync(&self, local_files: &HashSet<String>) -> Result<SyncResult> {
        let url = format!("{DB_BASE}/metadata/raw-lyrics-index.jsonl");
        info!("Downloading index from: {url}");

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("HTTP error downloading index: {}", resp.status());
        }

        let text = resp.text().await?;
        let mut remote_entries_map = std::collections::HashMap::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(raw_entry) = serde_json::from_str::<RawIndexEntry>(line) {
                remote_entries_map.insert(raw_entry.raw_lyric_file.clone(), raw_entry);
            }
        }

        let remote_files: HashSet<String> = remote_entries_map.keys().cloned().collect();
        let to_download: Vec<String> = remote_files.difference(local_files).cloned().collect();

        info!(
            "Remote files: {}, Local files: {}, To download: {}",
            remote_files.len(),
            local_files.len(),
            to_download.len()
        );

        if to_download.len() > INCREMENTAL_THRESHOLD {
            info!(
                "Too many files to download ({}), fallback to full sync",
                to_download.len()
            );
            anyhow::bail!("Incremental threshold exceeded");
        }

        if to_download.is_empty() {
            info!("No new files to download.");
            return Ok(SyncResult {
                status: SyncStatus::Skipped,
                count: Some(0),
                error: None,
                strategy: Some("incremental".to_string()),
            });
        }

        let client = self.client.clone();
        let fetched_models: Vec<entity::Model> = stream::iter(to_download)
            .map(|filename| {
                let client = client.clone();
                let raw_entry_opt = remote_entries_map.get(&filename).cloned();

                async move {
                    let url = format!("{DB_BASE}/raw-lyrics/{filename}");
                    if let Ok(resp) = client.get(&url).send().await
                        && resp.status().is_success()
                        && let Ok(content) = resp.text().await
                        && let Ok(result) = ttml_processor::parse_ttml(&content)
                    {
                        let mut model = build_entity_from_ttml(&filename, &content, &result);
                        if let Some(raw_entry) = raw_entry_opt {
                            merge_raw_index_entry(&mut model, raw_entry);
                        }
                        return Some(model);
                    }
                    None
                }
            })
            .buffer_unordered(CONCURRENT_WORKERS)
            .filter_map(|m| async move { m })
            .collect()
            .await;

        info!(
            "Successfully downloaded and parsed {} new entries",
            fetched_models.len()
        );

        self.batch_upsert_entries(&fetched_models).await?;

        Ok(SyncResult {
            status: SyncStatus::Updated,
            count: Some(fetched_models.len()),
            error: None,
            strategy: Some("incremental".to_string()),
        })
    }

    async fn batch_upsert_entries(&self, models: &[entity::Model]) -> Result<()> {
        for chunk in models.chunks(UPSERT_BATCH_SIZE) {
            let active_models: Vec<entity::ActiveModel> =
                chunk.iter().cloned().map(Into::into).collect();

            entity::Entity::insert_many(active_models)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::column(entity::Column::Filename)
                        .update_columns([
                            entity::Column::Timestamp,
                            entity::Column::TrackNames,
                            entity::Column::ArtistNames,
                            entity::Column::AlbumNames,
                            entity::Column::NormalizedTrackNames,
                            entity::Column::NormalizedArtistNames,
                            entity::Column::NormalizedAlbumNames,
                            entity::Column::NcmMusicIds,
                            entity::Column::QqMusicIds,
                            entity::Column::AppleMusicIds,
                            entity::Column::SpotifyIds,
                            entity::Column::Isrcs,
                            entity::Column::AuthorIds,
                            entity::Column::AuthorUsernames,
                            entity::Column::LyricText,
                            entity::Column::BgVocalText,
                            entity::Column::RawTtml,
                        ])
                        .to_owned(),
                )
                .exec(&self.db)
                .await?;
        }
        Ok(())
    }
}

pub fn filter_new_entries(
    models: Vec<entity::Model>,
    local_files: &HashSet<String>,
) -> Vec<entity::Model> {
    models
        .into_iter()
        .filter(|m| !local_files.contains(&m.filename))
        .collect()
}

pub fn build_entity_from_ttml(
    filename: &str,
    raw_ttml: &str,
    result: &ttml_processor::model::TTMLResult,
) -> entity::Model {
    let timestamp = filename
        .split('-')
        .next()
        .and_then(|ts_str| ts_str.parse::<i64>().ok())
        .unwrap_or(0);

    let id = generate_file_id(filename).cast_signed();
    let meta = &result.metadata;

    let flatten_vec = |v: &Option<Vec<String>>| -> serde_json::Value {
        serde_json::to_value(v.clone().unwrap_or_default()).unwrap_or_default()
    };

    let norm_vec = |v: &Option<Vec<String>>| -> serde_json::Value {
        let norm: Vec<String> = v
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|s| normalize_name_for_comparison(s))
            .collect();
        serde_json::to_value(norm).unwrap_or_default()
    };

    let lyric_text = result
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let bg_vocal_text = result
        .lines
        .iter()
        .filter_map(|l| l.background_vocal.as_ref())
        .map(|bg| bg.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    entity::Model {
        id,
        filename: filename.to_string(),
        timestamp,
        track_names: flatten_vec(&meta.title),
        artist_names: flatten_vec(&meta.artist),
        album_names: flatten_vec(&meta.album),
        normalized_track_names: norm_vec(&meta.title),
        normalized_artist_names: norm_vec(&meta.artist),
        normalized_album_names: norm_vec(&meta.album),
        ncm_music_ids: flatten_vec(&None),
        qq_music_ids: flatten_vec(&None),
        apple_music_ids: flatten_vec(&None),
        spotify_ids: flatten_vec(&None),
        isrcs: flatten_vec(&None),
        author_ids: flatten_vec(&meta.author_ids),
        author_usernames: flatten_vec(&meta.author_names),
        lyric_text,
        bg_vocal_text,
        raw_ttml: raw_ttml.to_string(),
    }
}

pub fn merge_raw_index_entry(model: &mut entity::Model, raw: RawIndexEntry) {
    let mut ncm = Vec::new();
    let mut qq = Vec::new();
    let mut apple = Vec::new();
    let mut spotify = Vec::new();
    let mut isrc = Vec::new();
    let mut author_ids = Vec::new();
    let mut author_usernames = Vec::new();

    for (key, values) in raw.metadata {
        match key.as_str() {
            "ncmMusicId" => ncm.extend(values),
            "qqMusicId" => qq.extend(values),
            "appleMusicId" => apple.extend(values),
            "spotifyId" => spotify.extend(values),
            "isrc" => isrc.extend(values),
            "ttmlAuthorGithub" => author_ids.extend(values),
            "ttmlAuthorGithubLogin" => author_usernames.extend(values),
            _ => {}
        }
    }

    if !ncm.is_empty() {
        model.ncm_music_ids = serde_json::to_value(ncm).unwrap_or_default();
    }
    if !qq.is_empty() {
        model.qq_music_ids = serde_json::to_value(qq).unwrap_or_default();
    }
    if !apple.is_empty() {
        model.apple_music_ids = serde_json::to_value(apple).unwrap_or_default();
    }
    if !spotify.is_empty() {
        model.spotify_ids = serde_json::to_value(spotify).unwrap_or_default();
    }
    if !isrc.is_empty() {
        model.isrcs = serde_json::to_value(isrc).unwrap_or_default();
    }
    if !author_ids.is_empty() {
        model.author_ids = serde_json::to_value(author_ids).unwrap_or_default();
    }
    if !author_usernames.is_empty() {
        model.author_usernames = serde_json::to_value(author_usernames).unwrap_or_default();
    }
}
