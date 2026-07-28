use std::env;

use axum::{
    Json,
    extract::State,
    http::HeaderMap,
};
use serde_json::{
    Value,
    json,
};
use tracing::info;

use crate::core::{
    error::AppError,
    state::AppState,
};

pub async fn handle_webhook_sync(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let expected_secret = env::var("SYNC_SECRET").map_err(|_| {
        AppError::InternalServerError(
            "SYNC_SECRET environment variable is not configured on the server.".to_string(),
        )
    })?;

    if expected_secret.trim().is_empty() {
        return Err(AppError::InternalServerError(
            "SYNC_SECRET environment variable is empty.".to_string(),
        ));
    }

    let auth_header = headers.get("Authorization").and_then(|h| h.to_str().ok());
    let expected_header = format!("Bearer {expected_secret}");

    if auth_header != Some(expected_header.as_str()) {
        return Err(AppError::Unauthorized);
    }

    info!("Webhook sync triggered via API");
    let state_clone = state;
    tokio::spawn(async move {
        if let Err(e) = state_clone.update_db().await {
            tracing::error!("Webhook triggered sync failed: {e:?}");
        }
    });

    Ok(Json(json!({
        "status": 200,
        "message": "Sync triggered"
    })))
}

#[cfg(test)]
mod tests {
    use std::env;

    use axum::{
        extract::State,
        http::HeaderMap,
    };

    use super::handle_webhook_sync;
    use crate::core::{
        error::AppError,
        state::AppState,
    };

    #[tokio::test]
    async fn test_webhook_sync_auth() {
        let state = AppState::new(None);

        unsafe {
            env::remove_var("SYNC_SECRET");
        }
        let headers_empty = HeaderMap::new();
        let res = handle_webhook_sync(headers_empty, State(state.clone())).await;
        assert!(matches!(res, Err(AppError::InternalServerError(_))));

        unsafe {
            env::set_var("SYNC_SECRET", "test_secret_123");
        }

        let mut headers = HeaderMap::new();
        let res = handle_webhook_sync(headers.clone(), State(state.clone())).await;
        assert!(matches!(res, Err(AppError::Unauthorized)));

        headers.insert("Authorization", "test_secret_123".parse().unwrap());
        let res = handle_webhook_sync(headers.clone(), State(state.clone())).await;
        assert!(matches!(res, Err(AppError::Unauthorized)));

        headers.insert("Authorization", "Bearer test_secret_123".parse().unwrap());
        let res = handle_webhook_sync(headers, State(state.clone())).await;
        assert!(res.is_ok());
        unsafe {
            env::remove_var("SYNC_SECRET");
        }
    }
}
