use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::HeaderMap,
};
use hmac::{
    Hmac,
    KeyInit,
    Mac,
};
use serde_json::{
    Value,
    json,
};
use sha2::Sha256;
use tracing::info;

use crate::core::{
    error::AppError,
    state::AppState,
};

type HmacSha256 = Hmac<Sha256>;

pub async fn handle_webhook_sync(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let expected_secret = state.sync_secret.as_deref().ok_or_else(|| {
        AppError::InternalServerError(
            "SYNC_SECRET environment variable is not configured on the server.".to_string(),
        )
    })?;

    if !verify_webhook_auth(&headers, &body, expected_secret) {
        return Err(AppError::Unauthorized);
    }

    let event_type = headers
        .get("X-GitHub-Event")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("unknown");

    if let Ok(payload_json) = serde_json::from_slice::<Value>(&body) {
        info!(
            event = %event_type,
            payload = %payload_json,
            "Webhook sync triggered via API"
        );
    } else if !body.is_empty() {
        let body_str = String::from_utf8_lossy(&body);
        info!(
            event = %event_type,
            body = %body_str,
            "Webhook sync triggered via API"
        );
    } else {
        info!(
            event = %event_type,
            "Webhook sync triggered via API with empty body"
        );
    }
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

fn verify_webhook_auth(headers: &HeaderMap, body: &[u8], secret: &str) -> bool {
    // Bearer Token
    if let Some(auth_header) = headers.get("Authorization").and_then(|h| h.to_str().ok()) {
        let expected_header = format!("Bearer {secret}");
        if auth_header == expected_header {
            return true;
        }
    }

    // GitHub 原生 X-Hub-Signature-256 HMAC-SHA256 签名
    if let Some(sig_header) = headers
        .get("X-Hub-Signature-256")
        .and_then(|h| h.to_str().ok())
        && let Some(hex_sig) = sig_header.strip_prefix("sha256=")
        && let Ok(expected_mac) = hex::decode(hex_sig)
        && let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes())
    {
        mac.update(body);
        if mac.verify_slice(&expected_mac).is_ok() {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Bytes,
        extract::State,
        http::HeaderMap,
    };
    use hmac::{
        Hmac,
        KeyInit,
        Mac,
    };
    use sha2::Sha256;

    use super::handle_webhook_sync;
    use crate::{
        core::{
            error::AppError,
            state::AppState,
        },
        init_db,
    };

    fn compute_github_signature(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let result = mac.finalize().into_bytes();
        format!("sha256={}", hex::encode(result))
    }

    #[tokio::test]
    async fn test_webhook_sync_auth_bearer() {
        let db_conn = init_db("sqlite::memory:").await.unwrap();

        let state_no_secret = AppState::new_with_secret(db_conn.clone(), None);
        let headers_empty = HeaderMap::new();
        let res = handle_webhook_sync(headers_empty, State(state_no_secret), Bytes::new()).await;
        assert!(matches!(res, Err(AppError::InternalServerError(_))));

        let state = AppState::new_with_secret(db_conn, Some("test_secret_123"));

        let mut headers = HeaderMap::new();
        let res = handle_webhook_sync(headers.clone(), State(state.clone()), Bytes::new()).await;
        assert!(matches!(res, Err(AppError::Unauthorized)));

        headers.insert("Authorization", "test_secret_123".parse().unwrap());
        let res = handle_webhook_sync(headers.clone(), State(state.clone()), Bytes::new()).await;
        assert!(matches!(res, Err(AppError::Unauthorized)));

        headers.insert("Authorization", "Bearer test_secret_123".parse().unwrap());
        let res = handle_webhook_sync(headers, State(state), Bytes::new()).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_webhook_sync_auth_github_hmac() {
        let db_conn = init_db("sqlite::memory:").await.unwrap();
        let secret = "github_webhook_secret_key";
        let state = AppState::new_with_secret(db_conn, Some(secret));

        let payload =
            Bytes::from_static(b"{\"action\": \"closed\", \"pull_request\": {\"merged\": true}}");
        let valid_sig = compute_github_signature(secret, &payload);

        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", valid_sig.parse().unwrap());
        let res = handle_webhook_sync(headers, State(state.clone()), payload.clone()).await;
        assert!(res.is_ok());

        let tampered_payload = Bytes::from_static(b"{\"action\": \"opened\"}");
        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", valid_sig.parse().unwrap());
        let res = handle_webhook_sync(headers, State(state.clone()), tampered_payload).await;
        assert!(matches!(res, Err(AppError::Unauthorized)));

        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", "sha1=invalid".parse().unwrap());
        let res = handle_webhook_sync(headers, State(state), payload).await;
        assert!(matches!(res, Err(AppError::Unauthorized)));
    }
}
