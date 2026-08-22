use axum::{
    Json,
    http::StatusCode,
    response::{
        IntoResponse,
        Response,
    },
};
use compact_str::CompactString;
use serde::{
    Serialize,
    Serializer,
};

use crate::core::{
    LyricId,
    error::AppError,
    models::SongEntry,
    pagination::PaginationInfo,
};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SongItem {
    pub id: LyricId,
    pub filename: String,

    pub music_names: Box<[CompactString]>,
    pub artist_names: Box<[CompactString]>,
    pub album_names: Box<[CompactString]>,

    pub ncm_music_ids: Box<[CompactString]>,
    pub qq_music_ids: Box<[CompactString]>,
    pub apple_music_ids: Box<[CompactString]>,
    pub spotify_ids: Box<[CompactString]>,

    pub isrcs: Box<[CompactString]>,

    pub author_ids: Box<[CompactString]>,
    pub author_usernames: Box<[CompactString]>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub lyrics: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_context: Option<MatchContext>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MatchContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchData {
    pub items: Vec<SongItem>,
    pub pagination: PaginationInfo,
}

pub struct HttpOk;

impl Serialize for HttpOk {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(200)
    }
}

#[derive(Serialize)]
pub struct SuccessResponse<T> {
    pub status: HttpOk,
    pub data: T,
}

pub struct ApiSuccess<T>(pub T);

impl<T: Serialize> IntoResponse for ApiSuccess<T> {
    fn into_response(self) -> Response {
        let envelope = SuccessResponse {
            status: HttpOk,
            data: self.0,
        };
        Json(envelope).into_response()
    }
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub status: u16,
    pub error: &'static str,
    pub message: String,
}

const INTERNAL_ERROR_MESSAGE: &str = "An internal error occurred.";
const UPSTREAM_ERROR_MESSAGE: &str = "Upstream service unavailable.";

fn error_parts(err: &AppError) -> (u16, &'static str, String) {
    match err {
        AppError::ReqwestError(_) | AppError::JsonError(_) | AppError::InternalServerError(_) => {
            (500, "Internal Server Error", INTERNAL_ERROR_MESSAGE.into())
        }
        AppError::NotFound => (
            404,
            "Not Found",
            "The requested API route does not exist.".into(),
        ),
        AppError::LyricNotFound => (
            404,
            "Not Found",
            "No lyrics found for the provided query.".into(),
        ),
        AppError::BadRequest(msg) => (400, "Bad Request", msg.clone()),
        AppError::Unauthorized => (
            401,
            "Unauthorized",
            "Invalid or missing authorization token.".into(),
        ),
        AppError::UpstreamError(_) => (502, "Bad Gateway", UPSTREAM_ERROR_MESSAGE.into()),
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_str, message) = error_parts(&self);

        if status >= 500 {
            tracing::error!(
                status,
                error = error_str,
                detail = ?self,
                "Request failed with server error"
            );
        }

        let payload = ErrorResponse {
            status,
            error: error_str,
            message,
        };

        (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(payload),
        )
            .into_response()
    }
}

pub fn map_song_to_item(
    song: &SongEntry,
    lyrics: Option<String>,
    format: Option<String>,
    match_context: Option<MatchContext>,
) -> SongItem {
    SongItem {
        id: song.id,
        filename: song.filename.to_string(),
        music_names: song.track_names.clone(),
        artist_names: song.artist_names.clone(),
        album_names: song.album_names.clone(),

        ncm_music_ids: song.ncm_music_ids.clone(),
        qq_music_ids: song.qq_music_ids.clone(),
        apple_music_ids: song.apple_music_ids.clone(),
        spotify_ids: song.spotify_ids.clone(),

        isrcs: song.isrcs.clone(),

        author_ids: song.author_ids.clone(),
        author_usernames: song.author_usernames.clone(),

        lyrics,
        format,

        match_context,
    }
}

#[cfg(test)]
mod tests {
    use compact_str::CompactString;

    use super::*;

    #[test]
    fn map_song_to_item_includes_id() {
        let filename = "1768754400682-250306205-r6IrpmBd.ttml";
        let song = SongEntry {
            id: LyricId::from_filename(filename),
            filename: CompactString::new(filename),
            timestamp: 1,
            track_names: vec![CompactString::new("ME!")].into_boxed_slice(),
            artist_names: Box::default(),
            album_names: Box::default(),
            normalized_track_names: vec![CompactString::new("me!")].into_boxed_slice(),
            normalized_artist_names: Box::default(),
            normalized_album_names: Box::default(),
            ncm_music_ids: Box::default(),
            qq_music_ids: Box::default(),
            apple_music_ids: Box::default(),
            spotify_ids: Box::default(),
            isrcs: Box::default(),
            author_ids: Box::default(),
            author_usernames: Box::default(),
        };

        let item = map_song_to_item(&song, None, None, None);
        assert_eq!(item.id, song.id);
        assert_eq!(item.filename, filename);

        let json = serde_json::to_value(&item).unwrap();
        assert!(json.get("id").unwrap().is_number());
        assert_eq!(json.get("id").unwrap().as_u64().unwrap(), song.id.get());
    }

    #[test]
    fn search_data_serialization_includes_nested_pagination() {
        let response = SuccessResponse {
            status: HttpOk,
            data: SearchData {
                items: vec![],
                pagination: PaginationInfo {
                    page: 1,
                    page_size: 20,
                    total: 156,
                    total_pages: 8,
                    has_more: true,
                },
            },
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["status"], 200);

        let data = &json["data"];
        assert!(data.get("items").unwrap().is_array());

        let pagination = &data["pagination"];
        assert_eq!(pagination["page"], 1);
        assert_eq!(pagination["pageSize"], 20);
        assert_eq!(pagination["total"], 156);
        assert_eq!(pagination["totalPages"], 8);
        assert_eq!(pagination["hasMore"], true);
    }

    #[test]
    fn song_item_omits_absent_optional_fields() {
        let filename = "1768754400682-250306205-r6IrpmBd.ttml";
        let song = SongEntry {
            id: LyricId::from_filename(filename),
            filename: CompactString::new(filename),
            timestamp: 1,
            track_names: vec![CompactString::new("ME!")].into_boxed_slice(),
            artist_names: Box::default(),
            album_names: Box::default(),
            normalized_track_names: Box::default(),
            normalized_artist_names: Box::default(),
            normalized_album_names: Box::default(),
            ncm_music_ids: Box::default(),
            qq_music_ids: Box::default(),
            apple_music_ids: Box::default(),
            spotify_ids: Box::default(),
            isrcs: Box::default(),
            author_ids: Box::default(),
            author_usernames: Box::default(),
        };

        let bare = map_song_to_item(&song, None, None, None);
        let json = serde_json::to_value(&bare).unwrap();
        assert!(json.get("lyrics").is_none());
        assert!(json.get("format").is_none());
        assert!(json.get("matchContext").is_none());

        let full = map_song_to_item(
            &song,
            Some("<tt/>".to_string()),
            Some("ttml".to_string()),
            Some(MatchContext {
                snippet: Some("a <mark>b</mark> c".to_string()),
            }),
        );
        let json = serde_json::to_value(&full).unwrap();
        assert_eq!(json["lyrics"], "<tt/>");
        assert_eq!(json["format"], "ttml");
        assert_eq!(json["matchContext"]["snippet"], "a <mark>b</mark> c");
    }

    #[test]
    fn bad_request_message_is_client_facing() {
        let err = AppError::BadRequest("Missing params".into());
        let (status, error, message) = error_parts(&err);
        assert_eq!((status, error), (400, "Bad Request"));
        assert_eq!(message, "Missing params");
    }

    #[test]
    fn not_found_messages_are_client_facing() {
        let (status, error, message) = error_parts(&AppError::NotFound);
        assert_eq!((status, error), (404, "Not Found"));
        assert_eq!(message, "The requested API route does not exist.");

        let (status, _, message) = error_parts(&AppError::LyricNotFound);
        assert_eq!(status, 404);
        assert_eq!(message, "No lyrics found for the provided query.");
    }

    #[test]
    fn server_error_messages_are_generic() {
        let (status, _, message) = error_parts(&AppError::InternalServerError(
            "FTS Query Error: database is locked".into(),
        ));
        assert_eq!(status, 500);
        assert_eq!(message, "An internal error occurred.");

        let (status, _, message) = error_parts(&AppError::UpstreamError(
            "context: Failed to download zip: connection reset".into(),
        ));
        assert_eq!(status, 502);
        assert_eq!(message, "Upstream service unavailable.");
    }

    #[test]
    fn error_response_serialization() {
        let (status, error, message) = error_parts(&AppError::BadRequest("test".into()));
        let payload = ErrorResponse {
            status,
            error,
            message,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert_eq!(
            json,
            r#"{"status":400,"error":"Bad Request","message":"test"}"#
        );
    }
}
