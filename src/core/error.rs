use axum::{
    Json,
    http::StatusCode,
    response::{
        IntoResponse,
        Response,
    },
};
use serde::Serialize;

#[derive(Debug)]
pub enum AppError {
    ReqwestError(reqwest::Error),
    JsonError(serde_json::Error),
    NotFound,
    LyricNotFound,
    BadRequest(String),
    UpstreamError(String),
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        Self::ReqwestError(err)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        Self::JsonError(err)
    }
}

#[derive(Serialize)]
struct ErrorPayload {
    status: u16,
    error: &'static str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_str, message) = self.parts();

        let payload = ErrorPayload {
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

impl AppError {
    pub fn parts(&self) -> (u16, &'static str, String) {
        match self {
            Self::ReqwestError(err) => (500, "Internal Server Error", err.to_string()),
            Self::JsonError(err) => (500, "Internal Server Error", err.to_string()),
            Self::NotFound => (
                404,
                "Not Found",
                "The requested API route does not exist.".into(),
            ),
            Self::LyricNotFound => (
                404,
                "Not Found",
                "No lyrics found for the provided query.".into(),
            ),
            Self::BadRequest(msg) => (400, "Bad Request", msg.clone()),
            Self::UpstreamError(msg) => (502, "Bad Gateway", msg.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_request_parts() {
        let err = AppError::BadRequest("Missing params".into());
        let (status, error, message) = err.parts();
        assert_eq!(status, 400);
        assert_eq!(error, "Bad Request");
        assert_eq!(message, "Missing params");
    }

    #[test]
    fn not_found_parts() {
        let err = AppError::NotFound;
        let (status, error, message) = err.parts();
        assert_eq!(status, 404);
        assert_eq!(error, "Not Found");
        assert_eq!(message, "The requested API route does not exist.");
    }

    #[test]
    fn lyric_not_found_parts() {
        let err = AppError::LyricNotFound;
        let (status, error, message) = err.parts();
        assert_eq!(status, 404);
        assert_eq!(error, "Not Found");
        assert_eq!(message, "No lyrics found for the provided query.");
    }

    #[test]
    fn upstream_error_parts() {
        let err = AppError::UpstreamError("GitHub down".into());
        let (status, error, message) = err.parts();
        assert_eq!(status, 502);
        assert_eq!(error, "Bad Gateway");
        assert_eq!(message, "GitHub down");
    }

    #[test]
    fn error_payload_serialization() {
        let err = AppError::BadRequest("test".into());
        let (status, error, message) = err.parts();
        let payload = ErrorPayload {
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
