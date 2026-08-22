#[derive(Debug)]
pub enum AppError {
    ReqwestError(reqwest::Error),
    JsonError(serde_json::Error),
    NotFound,
    LyricNotFound,
    BadRequest(String),
    Unauthorized,
    InternalServerError(String),
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
