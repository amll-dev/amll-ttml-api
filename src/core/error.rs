use serde::Serialize;
use worker::{
    Response,
    Result,
};

#[derive(Debug)]
pub enum AppError {
    WorkerError(worker::Error),
    JsonError(serde_json::Error),
    NotFound,
    LyricNotFound,
    BadRequest(String),
    UpstreamError(String),
}

impl From<worker::Error> for AppError {
    fn from(err: worker::Error) -> Self {
        Self::WorkerError(err)
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

impl AppError {
    pub fn to_response(&self) -> Result<Response> {
        let (status, error_str, message) = match self {
            Self::WorkerError(err) => (500, "Internal Server Error", err.to_string()),
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
        };

        let payload = ErrorPayload {
            status,
            error: error_str,
            message,
        };

        let res = Response::from_json(&payload)?;

        Ok(res.with_status(status))
    }

    #[cfg(test)]
    fn parts(&self) -> (u16, &'static str, String) {
        match self {
            Self::WorkerError(err) => (500, "Internal Server Error", err.to_string()),
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
