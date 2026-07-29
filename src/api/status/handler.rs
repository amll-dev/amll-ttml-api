use axum::{
    Json,
    extract::State,
    response::IntoResponse,
};
use serde::Serialize;

use crate::core::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    pub commit: &'static str,
    pub commit_date: &'static str,
    pub build_time: &'static str,
    pub rustc_version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub uptime_seconds: u64,
    pub lyric_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub build: BuildInfo,
    pub runtime: RuntimeStatus,
}

pub async fn handle_status(State(state): State<AppState>) -> impl IntoResponse {
    let uptime_seconds = state.start_time.elapsed().as_secs();
    let lyric_count = state.lyric_count();

    let response = StatusResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        build: BuildInfo {
            commit: env!("GIT_HASH"),
            commit_date: env!("GIT_COMMIT_DATE"),
            build_time: env!("BUILD_TIME"),
            rustc_version: env!("RUSTC_VERSION"),
        },
        runtime: RuntimeStatus {
            uptime_seconds,
            lyric_count,
        },
    };

    Json(response)
}

#[cfg(test)]
mod tests {
    use axum::{
        extract::State,
        response::IntoResponse,
    };

    use super::handle_status;
    use crate::{
        AppState,
        init_db,
    };

    #[tokio::test]
    async fn test_status_endpoint() {
        let db_conn = init_db("sqlite::memory:").await.unwrap();
        let state = AppState::new(db_conn);
        let response = handle_status(State(state)).await.into_response();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
