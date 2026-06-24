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
}
