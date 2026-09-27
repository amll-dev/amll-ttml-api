//! 内部运维端点：团队下载请求统计的每日 Parquet 文件
//!
//! 不写进公开接口文档。用 `ANALYTICS_SECRET` 做 Bearer 鉴权；所有响应（含错误）一律 `no-store`，
//! 带着 token 的内容不该留在任何缓存里

use axum::{
    body::Body,
    extract::{
        Path,
        Request,
        State,
    },
    http::{
        HeaderMap,
        HeaderValue,
        header::{
            AUTHORIZATION,
            CACHE_CONTROL,
            CONTENT_DISPOSITION,
            CONTENT_TYPE,
        },
    },
    response::{
        IntoResponse,
        Response,
    },
};
use sha2::{
    Digest,
    Sha256,
};
use subtle::ConstantTimeEq;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::{
    analytics::Analytics,
    api::{
        admin::dto::{
            AnalyticsFilesData,
            map_daily_file,
        },
        shared::{
            cache::NO_STORE_CACHE_CONTROL,
            dto::ApiSuccess,
        },
    },
    core::error::AppError,
    services::AppState,
};

const PARQUET_CONTENT_TYPE: HeaderValue =
    HeaderValue::from_static("application/vnd.apache.parquet");

/// 让 nginx 边读边发，而不是先把整个文件缓冲到自己的临时目录（会占服务器磁盘）
const ACCEL_BUFFERING: &str = "x-accel-buffering";

/// `GET /v1/admin/analytics/files`：可供下载的每日文件清单
pub async fn handle_list_files(State(state): State<AppState>, headers: HeaderMap) -> Response {
    no_store(list_files(&state, &headers).await.into_response())
}

/// `GET /v1/admin/analytics/files/{name}`：下载一个每日文件，支持 Range 断点续传
pub async fn handle_download_file(
    State(state): State<AppState>,
    Path(name): Path<String>,
    request: Request,
) -> Response {
    no_store(download_file(&state, &name, request).await.into_response())
}

async fn list_files(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ApiSuccess<AnalyticsFilesData>, AppError> {
    let analytics = authorize(state, headers)?;

    let files = analytics.daily_files().await.map_err(|e| {
        AppError::InternalServerError(format!("Failed to list analytics files: {e}"))
    })?;

    Ok(ApiSuccess(AnalyticsFilesData {
        files: files.into_iter().map(map_daily_file).collect(),
    }))
}

async fn download_file(
    state: &AppState,
    name: &str,
    request: Request,
) -> Result<Response, AppError> {
    let analytics = authorize(state, request.headers())?;

    let path = analytics.daily_file_path(name).ok_or(AppError::NotFound)?;
    // ServeFile 对缺失文件返回自己的空 404，先查一遍，保持统一的错误形状
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(AppError::NotFound);
    }

    let response = ServeFile::new(&path)
        .oneshot(request)
        .await
        .map_err(|e| AppError::InternalServerError(format!("Failed to serve {name}: {e}")))?;

    let mut response = response.map(Body::new);
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, PARQUET_CONTENT_TYPE);
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(CONTENT_DISPOSITION, disposition);
    }
    headers.insert(ACCEL_BUFFERING, HeaderValue::from_static("no"));

    Ok(response)
}

/// 统计未启用时端点等同于不存在；启用了但没配密钥是服务端配置错误
fn authorize<'a>(state: &'a AppState, headers: &HeaderMap) -> Result<&'a Analytics, AppError> {
    let analytics = state.analytics.as_ref().ok_or(AppError::NotFound)?;

    let secret = analytics.secret().ok_or_else(|| {
        AppError::InternalServerError(
            "ANALYTICS_SECRET environment variable is not configured on the server.".to_string(),
        )
    })?;

    let provided = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;

    if token_matches(provided, secret) {
        Ok(analytics)
    } else {
        Err(AppError::Unauthorized)
    }
}

/// 常量时间比较。先各自取哈希，长度差异也不会从耗时上泄露
fn token_matches(provided: &str, expected: &str) -> bool {
    let provided = Sha256::digest(provided.as_bytes());
    let expected = Sha256::digest(expected.as_bytes());
    provided.as_slice().ct_eq(expected.as_slice()).into()
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, NO_STORE_CACHE_CONTROL);
    response
}

#[cfg(test)]
mod tests {
    use super::token_matches;

    #[test]
    fn token_comparison() {
        assert!(token_matches("secret", "secret"));
        assert!(!token_matches("secret ", "secret"));
        assert!(!token_matches("", "secret"));
        assert!(!token_matches("SECRET", "secret"));
    }
}
