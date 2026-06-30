use worker::Url;

use crate::core::{
    error::AppError,
    models::IdQuery,
};

pub struct GetQuery {
    pub id_query: IdQuery,
    pub format: String,
}

pub fn extract_get_query(url: &Url) -> Result<GetQuery, AppError> {
    let mut query = IdQuery::default();
    let mut has_param = false;
    let mut format = String::from("ttml");

    for (k, v) in url.query_pairs() {
        let val = v.into_owned();
        // 空字符串视为未传入
        if val.trim().is_empty() {
            continue;
        }
        match k.as_ref() {
            "filename" => {
                query.filename = Some(val);
                has_param = true;
            }
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
            "format" => {
                format = val;
            }
            _ => {}
        }
    }

    if format != "ttml" {
        return Err(AppError::BadRequest(format!(
            "Unsupported format: '{format}'. Only 'ttml' is currently supported."
        )));
    }

    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    if let Some(ref filename) = query.filename
        && !filename.ends_with(".ttml")
    {
        return Err(AppError::BadRequest(format!(
            "Invalid filename: '{filename}'. Must end with '.ttml'."
        )));
    }

    if has_param {
        Ok(GetQuery {
            id_query: query,
            format,
        })
    } else {
        Err(AppError::BadRequest(
            "At least one parameter is required (filename, ncmMusicId, qqMusicId, appleMusicId, spotifyId, isrc).".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn no_params_returns_error() {
        let result = extract_get_query(&url("https://example.com/api/v1/lyrics/get"));
        assert!(result.is_err());
    }

    #[test]
    fn empty_id_returns_error() {
        let result = extract_get_query(&url("https://example.com/api/v1/lyrics/get?spotifyId="));
        assert!(result.is_err());
    }

    #[test]
    fn format_only_returns_error() {
        let result = extract_get_query(&url("https://example.com/api/v1/lyrics/get?format=ttml"));
        assert!(result.is_err());
    }

    #[test]
    fn single_spotify_id() {
        let result = extract_get_query(&url(
            "https://example.com/api/v1/lyrics/get?spotifyId=abc123",
        ))
        .unwrap();
        assert_eq!(result.id_query.spotify_ids, vec!["abc123"]);
        assert_eq!(result.format, "ttml");
    }

    #[test]
    fn default_format_is_ttml() {
        let result =
            extract_get_query(&url("https://example.com/api/v1/lyrics/get?ncmMusicId=111"))
                .unwrap();
        assert_eq!(result.format, "ttml");
    }

    #[test]
    fn unsupported_format_returns_error() {
        let result = extract_get_query(&url(
            "https://example.com/api/v1/lyrics/get?spotifyId=abc&format=lrc",
        ));
        assert!(result.is_err());
    }

    #[test]
    fn multiple_ids_same_type() {
        let result = extract_get_query(&url(
            "https://example.com/api/v1/lyrics/get?ncmMusicId=111&ncmMusicId=222",
        ))
        .unwrap();
        assert_eq!(result.id_query.ncm_music_ids, vec!["111", "222"]);
    }

    #[test]
    fn multiple_ids_different_types() {
        let result = extract_get_query(&url(
            "https://example.com/api/v1/lyrics/get?ncmMusicId=111&spotifyId=abc&isrc=XYZ",
        ))
        .unwrap();
        assert_eq!(result.id_query.ncm_music_ids, vec!["111"]);
        assert_eq!(result.id_query.spotify_ids, vec!["abc"]);
        assert_eq!(result.id_query.isrcs, vec!["XYZ"]);
    }

    #[test]
    fn all_id_types() {
        let result = extract_get_query(&url(
            "https://example.com/api/v1/lyrics/get?ncmMusicId=a&qqMusicId=b&appleMusicId=c&spotifyId=d&isrc=e",
        ))
        .unwrap();
        assert_eq!(result.id_query.ncm_music_ids, vec!["a"]);
        assert_eq!(result.id_query.qq_music_ids, vec!["b"]);
        assert_eq!(result.id_query.apple_music_ids, vec!["c"]);
        assert_eq!(result.id_query.spotify_ids, vec!["d"]);
        assert_eq!(result.id_query.isrcs, vec!["e"]);
    }

    #[test]
    fn filename_only() {
        let result = extract_get_query(&url(
            "https://example.com/api/v1/lyrics/get?filename=1768754400682-250306205-r6IrpmBd.ttml",
        ))
        .unwrap();
        assert_eq!(
            result.id_query.filename,
            Some("1768754400682-250306205-r6IrpmBd.ttml".into())
        );
        assert!(result.id_query.ncm_music_ids.is_empty());
    }

    #[test]
    fn filename_with_other_ids_ignored() {
        let result = extract_get_query(&url(
            "https://example.com/api/v1/lyrics/get?filename=a.ttml&ncmMusicId=111",
        ))
        .unwrap();
        assert_eq!(result.id_query.filename, Some("a.ttml".into()));
        assert_eq!(result.id_query.ncm_music_ids, vec!["111"]);
    }

    #[test]
    fn empty_filename_returns_error() {
        let result = extract_get_query(&url("https://example.com/api/v1/lyrics/get?filename="));
        assert!(result.is_err());
    }

    #[test]
    fn invalid_filename_extension_returns_error() {
        let result = extract_get_query(&url(
            "https://example.com/api/v1/lyrics/get?filename=1768754400682-250306205-r6IrpmBd.lrc",
        ));
        assert!(result.is_err());
    }
}
