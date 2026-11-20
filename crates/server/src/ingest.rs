//! `POST /i/v1/batch?k=<public key>`: recordings from the browser SDK. Outside `/api`, since it is
//! called cross-origin by design. The SDK sends gzip as `text/plain` with the key in the query, a
//! CORS simple request, so there is no preflight; `OPTIONS` still answers for clients that send headers.

use axum::body::{Body, to_bytes};
use axum::extract::{RawQuery, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentrail_core::recording::ingest::{self, IngestError, MAX_BODY, Outcome, Refusal};

use crate::AppState;
use crate::error::json_error;
use crate::security::{self, Limit};

/// A tab sends a batch every few seconds; an office behind one address has many tabs.
pub const INGEST: Limit = Limit { name: "ingest", per_minute: 1200, burst: 300 };

fn key_of(query: Option<&str>, headers: &HeaderMap) -> String {
    let from_query = query.and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("k=")).map(str::to_string));
    from_query.or_else(|| headers.get("x-trail-key").and_then(|v| v.to_str().ok()).map(str::to_string)).unwrap_or_default()
}

fn origin_of(headers: &HeaderMap) -> Option<String> {
    headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(str::to_string)
}

fn cors(mut response: Response, origin: Option<&str>) -> Response {
    let h = response.headers_mut();
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("cross-origin"));
    h.insert(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(value) = origin.and_then(|o| HeaderValue::from_str(o).ok()) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    response
}

pub async fn batch(State(state): State<AppState>, RawQuery(query): RawQuery, request: Request) -> Response {
    let headers = request.headers().clone();
    let origin = origin_of(&headers);
    let ip = security::client_ip(&state, &headers, security::peer(&request)).await;
    if !state.limiter.allow(INGEST, ip) {
        return cors(security::too_many(), origin.as_deref());
    }
    let key = key_of(query.as_deref(), &headers);
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY + 1).await else {
        return cors(json_error(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "The batch is too large"), origin.as_deref());
    };
    let user_agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    let result = ingest::ingest(
        &state.app,
        ingest::Request { key: &key, origin: origin.as_deref(), ip, user_agent, body: &body, now: opentrail_core::context::now() },
    )
    .await;
    let response = match result {
        Ok(Outcome::Stored | Outcome::Duplicate | Outcome::Truncated) => StatusCode::NO_CONTENT.into_response(),
        Err(IngestError::Refused(refusal)) => {
            tracing::info!(?refusal, origin = origin.as_deref().unwrap_or(""), "ingest refused");
            let (status, code, message) = match refusal {
                Refusal::Invalid(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
                Refusal::UnknownKey => (StatusCode::UNAUTHORIZED, "unknown_key", "Unknown or revoked key".into()),
                Refusal::Origin => {
                    // Without the CORS header the page cannot read why, which is the point.
                    return json_error(StatusCode::FORBIDDEN, "origin", "This origin may not send recordings for this key");
                }
                Refusal::Archived => (StatusCode::FORBIDDEN, "archived", "The project is archived".into()),
                Refusal::OtherProject => (StatusCode::FORBIDDEN, "forbidden", "The session belongs to another project".into()),
                Refusal::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "too_large", "The batch is too large".into()),
                Refusal::Quota => {
                    let mut r = json_error(StatusCode::TOO_MANY_REQUESTS, "quota", "The project's monthly session quota is used up");
                    r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("3600"));
                    return cors(r, origin.as_deref());
                }
            };
            json_error(status, code, &message)
        }
        Err(IngestError::Internal(e)) => {
            tracing::error!(error = %e, cause = ?e.cause, "ingest failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong on our side")
        }
    };
    cors(response, origin.as_deref())
}

pub async fn preflight(State(state): State<AppState>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    let origin = origin_of(&headers);
    let key = key_of(query.as_deref(), &headers);
    let allowed = match (&origin, ingest::allowed_origins(&state.app, &key).await) {
        (Some(o), Ok(Some(origins))) => origins.iter().any(|a| a.eq_ignore_ascii_case(o)),
        _ => false,
    };
    if !allowed {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    let h = response.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("POST"));
    h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type, x-trail-key"));
    h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("7200"));
    cors(response, origin.as_deref())
}
