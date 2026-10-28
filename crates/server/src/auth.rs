//! Every request (except the one unauthenticated health check) must carry
//! the session token minted at boot -- either `?token=...` (the only
//! option for things the browser loads directly: `<img>`/`<video>` `src`,
//! and `EventSource` for SSE, which the Fetch/XHR header APIs don't cover)
//! or an `Authorization: Bearer ...` header (for `fetch()`-based calls).
//! This is the thing that stops another local process, or a malicious
//! webpage open in the same browser, from reading someone's photo library
//! just because it can guess a loopback port is open.

use crate::state::AppState;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use std::collections::HashMap;

pub async fn require_token(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let query_token = params.get("token").map(|s| s.as_str());
    let header_token = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));

    let ok = query_token == Some(&*state.token) || header_token == Some(&*state.token);
    if ok {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// A fresh, hard-to-guess token for one server lifetime. Not meant to
/// resist a determined cryptographic attacker (there's no secret worth
/// that here) -- just long and random enough that no other local process
/// or page can stumble onto it.
pub fn new_token() -> String {
    use rand::Rng;
    let bytes: [u8; 24] = rand::thread_rng().gen();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_and_long_enough() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 48); // 24 bytes, hex-encoded
    }
}
