use std::{
    collections::HashMap,
    sync::{
        Arc,
        OnceLock,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
};

use async_lock::{
    Mutex,
    RwLock,
    RwLockReadGuard,
};
use compact_str::CompactString;
use worker::{
    Cache,
    Fetch,
    Method,
    Request,
    Response,
    RouteContext,
};

use crate::core::{
    error::AppError,
    models::{
        LyricIndexDB,
        RawIndexEntry,
        SongEntry,
    },
};

pub struct AppState {
    pub db: LyricIndexDB,
    pub last_updated_ms: u64,
}

static GLOBAL_STATE: OnceLock<Arc<RwLock<AppState>>> = OnceLock::new();
static INIT_MUTEX: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
static IS_UPDATING: AtomicBool = AtomicBool::new(false);

const INDEX_URL: &str =
    "https://raw.githubusercontent.com/amll-dev/amll-ttml-db/main/metadata/raw-lyrics-index.jsonl";

async fn fetch_and_parse_db() -> Result<LyricIndexDB, AppError> {
    let cache = Cache::default();
    let req = Request::new(INDEX_URL, Method::Get)?;

    let mut response = if let Some(cached_res) = cache.get(&req, true).await? {
        cached_res
    } else {
        let mut fetched_res = Fetch::Url(INDEX_URL.parse().unwrap()).send().await?;
        if !(200..=299).contains(&fetched_res.status_code()) {
            return Err(AppError::UpstreamError(
                "Failed to fetch index from GitHub".into(),
            ));
        }

        let mut response_to_cache = fetched_res.cloned()?;
        response_to_cache
            .headers_mut()
            .set("cache-control", "s-maxage=3600")?;
        cache.put(&req, response_to_cache).await?;
        fetched_res
    };

    let text = response.text().await?;
    let mut entries: Vec<SongEntry> = Vec::new();

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

        if let Ok(raw_entry) = serde_json::from_str::<RawIndexEntry>(line) {
            let song = SongEntry::from(raw_entry);

            let current_index = entries.len();

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
                author_id_idx.entry(id.clone()).or_default().push(current_index);
            }
            for id in &song.author_usernames {
                author_username_idx.entry(id.clone()).or_default().push(current_index);
            }

            entries.push(song);
        }
    }

    Ok(LyricIndexDB {
        entries,
        ncm_idx,
        qq_idx,
        apple_idx,
        spotify_idx,
        isrc_idx,
        author_id_idx,
        author_username_idx,
    })
}

pub async fn init_db_if_needed() -> Result<(), AppError> {
    if GLOBAL_STATE.get().is_some() {
        return Ok(());
    }

    let mutex = INIT_MUTEX.get_or_init(|| Arc::new(Mutex::new(())));
    let _guard = mutex.lock().await;

    if GLOBAL_STATE.get().is_some() {
        return Ok(());
    }

    let db = fetch_and_parse_db().await?;
    let state = AppState {
        db,
        last_updated_ms: worker::Date::now().as_millis(),
    };

    let _ = GLOBAL_STATE.set(Arc::new(RwLock::new(state)));
    Ok(())
}

pub async fn background_revalidate_db() {
    let result = fetch_and_parse_db().await;

    match result {
        Ok(new_db) => {
            if let Some(state_arc) = GLOBAL_STATE.get() {
                let mut state = state_arc.write().await;
                state.db = new_db;
                state.last_updated_ms = worker::Date::now().as_millis();
            }
        }
        Err(e) => {
            worker::console_error!("Background SWR update failed: {e:?}");
        }
    }

    IS_UPDATING.store(false, Ordering::Release);
}

pub async fn fetch_lyric_ttml(filename: &str) -> Result<String, AppError> {
    let ttml_url = format!(
        "https://raw.githubusercontent.com/amll-dev/amll-ttml-db/main/raw-lyrics/{filename}"
    );
    let cache = Cache::default();
    let ttml_req = Request::new(&ttml_url, Method::Get)?;

    let ttml_text = if let Some(mut cached_res) = cache.get(&ttml_req, true).await? {
        cached_res.text().await?
    } else {
        let mut fetched_res = Fetch::Url(ttml_url.parse().unwrap()).send().await?;
        if !(200..=299).contains(&fetched_res.status_code()) {
            return Err(AppError::UpstreamError(
                "Failed to fetch lyric file from GitHub".into(),
            ));
        }

        let text = fetched_res.text().await?;
        let mut response_to_cache = Response::ok(&text)?;
        response_to_cache
            .headers_mut()
            .set("cache-control", "s-maxage=604800")?;
        let _ = cache.put(&ttml_req, response_to_cache).await;
        text
    };

    Ok(ttml_text)
}

pub async fn acquire_db_read_lock(
    ctx: &RouteContext<worker::Context>,
) -> Result<RwLockReadGuard<'static, AppState>, AppError> {
    init_db_if_needed().await?;

    let state_arc = GLOBAL_STATE.get().unwrap();

    let state = state_arc.read().await;

    let current_ms = worker::Date::now().as_millis();
    if current_ms - state.last_updated_ms > 3_600_000
        && IS_UPDATING
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    {
        ctx.data.wait_until(async move {
            background_revalidate_db().await;
        });
    }

    Ok(state)
}
