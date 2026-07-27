use std::collections::HashMap;

use compact_str::CompactString;
use reqwest::Client;

use crate::core::{
    error::AppError,
    models::{
        LyricIndexDB,
        RawIndexEntry,
        SongEntry,
    },
};

const INDEX_URL: &str =
    "https://raw.githubusercontent.com/amll-dev/amll-ttml-db/main/metadata/raw-lyrics-index.jsonl";

pub async fn fetch_and_parse_db(client: &Client) -> Result<LyricIndexDB, AppError> {
    let res = client.get(INDEX_URL).send().await?;
    if !res.status().is_success() {
        return Err(AppError::UpstreamError(
            "Failed to fetch index from GitHub".into(),
        ));
    }

    let text = res.text().await?;
    let mut entries: Vec<SongEntry> = Vec::new();
    let mut id_idx: HashMap<u64, usize> = HashMap::new();

    let mut ncm_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
    let mut qq_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
    let mut apple_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
    let mut spotify_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
    let mut isrc_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
    let mut author_id_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
    let mut author_username_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match serde_json::from_str::<RawIndexEntry>(line) {
            Ok(raw_entry) => {
                let song = SongEntry::from(raw_entry);

                let current_index = entries.len();

                id_idx.insert(song.id, current_index);

                for id in &song.ncm_music_ids {
                    ncm_idx.entry(id.clone()).or_default().push(current_index);
                }
                for id in &song.qq_music_ids {
                    qq_idx.entry(id.clone()).or_default().push(current_index);
                }
                for id in &song.apple_music_ids {
                    apple_idx.entry(id.clone()).or_default().push(current_index);
                }
                for id in &song.spotify_ids {
                    spotify_idx
                        .entry(id.clone())
                        .or_default()
                        .push(current_index);
                }
                for id in &song.isrcs {
                    isrc_idx.entry(id.clone()).or_default().push(current_index);
                }
                for id in &song.author_ids {
                    author_id_idx
                        .entry(id.clone())
                        .or_default()
                        .push(current_index);
                }
                for id in &song.author_usernames {
                    author_username_idx
                        .entry(id.clone())
                        .or_default()
                        .push(current_index);
                }

                entries.push(song);
            }
            Err(err) => {
                tracing::warn!("Failed to parse JSONL line from GitHub index: {err}, line: {line}");
            }
        }
    }

    Ok(LyricIndexDB {
        entries,
        id_idx,
        ncm_idx,
        qq_idx,
        apple_idx,
        spotify_idx,
        isrc_idx,
        author_id_idx,
        author_username_idx,
    })
}

pub async fn fetch_raw_lyric(client: &Client, filename: &str) -> Result<String, AppError> {
    let ttml_url = format!(
        "https://raw.githubusercontent.com/amll-dev/amll-ttml-db/main/raw-lyrics/{filename}"
    );

    let res = client.get(&ttml_url).send().await?;
    if !res.status().is_success() {
        return Err(AppError::UpstreamError(
            "Failed to fetch lyric file from GitHub".into(),
        ));
    }

    let text = res.text().await?;
    Ok(text)
}
