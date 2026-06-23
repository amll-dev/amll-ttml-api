use worker::Url;

use crate::core::{
    error::AppError,
    models::IdQuery,
};

pub fn extract_id_query(url: &Url) -> Result<IdQuery, AppError> {
    let mut query = IdQuery::default();
    let mut has_param = false;

    for (k, v) in url.query_pairs() {
        let val = v.into_owned();
        match k.as_ref() {
            "ncmMusicId" => {
                query.ncm_music_ids.push(val);
                has_param = true;
            }
            "qqMusicId" => {
                query.qq_music_ids.push(val);
                has_param = true;
            }
            "appleMusicId" => {
                query.apple_music_ids.push(val);
                has_param = true;
            }
            "spotifyId" => {
                query.spotify_ids.push(val);
                has_param = true;
            }
            "isrc" => {
                query.isrcs.push(val);
                has_param = true;
            }
            _ => {}
        }
    }

    if has_param {
        Ok(query)
    } else {
        Err(AppError::BadRequest(
            "Only valid query parameters are allowed.".into(),
        ))
    }
}
