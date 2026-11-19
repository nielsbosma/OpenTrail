use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentrail_core::context::Surface;
use serde::Deserialize;

use crate::error::ApiError;
use crate::{AppState, RequestId, auth, security};

#[derive(Deserialize)]
pub struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}

/// `GET /api/auth/sso/callback`: the identity provider returns the browser here. A refusal goes to
/// the sign-in page as a fixed code it has a text for.
pub async fn callback(State(app_state): State<AppState>, Query(q): Query<Callback>, parts: Parts) -> Response {
    let request_id = parts.extensions.get::<RequestId>().map(|r| r.0.clone()).unwrap_or_default();
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = security::client_ip(&app_state, &parts.headers, peer).await;
    if !app_state.limiter.allow(security::AUTH, ip) {
        return security::too_many();
    }
    let ctx = match auth::context(&app_state, &parts.headers, ip.map(|i| i.to_string()), Surface::Web, request_id.clone()).await {
        Ok(ctx) => ctx,
        Err(e) => return ApiError(e, Some(request_id)).into_response(),
    };
    let browser = auth::sso_cookie(&parts.headers, &app_state);
    let outcome = opentrail_core::services::sso::callback(
        &ctx,
        q.state.as_deref().unwrap_or_default(),
        browser.as_deref(),
        q.code.as_deref(),
        q.error.as_deref(),
    )
    .await;
    let mut response = StatusCode::SEE_OTHER.into_response();
    let location = match outcome {
        Ok(session) => {
            response.headers_mut().append(header::SET_COOKIE, auth::set_cookie(&app_state, &session.token, session.expires_at));
            "/".to_string()
        }
        Err(code) => format!("/login?sso={code}"),
    };
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().append(header::SET_COOKIE, auth::clear_sso_cookie(&app_state));
    response
}
