use std::collections::HashMap;

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{MethodFilter, get, on};
use opentrail_core::context::{Credential, Ctx, Surface};
use opentrail_core::error::{AppError, ErrorKind, bad_request};
use opentrail_crypto::{KeyPurpose, sha256_hex};
use serde_json::{Map, Value, json};

use crate::error::{ApiError, json_error};
use crate::security;
use crate::{AppState, RequestId, auth};

/// One REST operation: a path over a service. Path parameters become input fields of the same name;
/// `fixed` fields are set by the route itself. `creates` answers 201 instead of 200.
pub struct Route {
    pub method: Method,
    pub path: &'static str,
    pub service: &'static str,
    pub fixed: &'static [(&'static str, bool)],
    pub creates: bool,
}

const fn route(method: Method, path: &'static str, service: &'static str) -> Route {
    let creates = matches!(method, Method::POST) && !has_parameter(path);
    Route { method, path, service, fixed: &[], creates }
}

const fn has_parameter(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            return true;
        }
        i += 1;
    }
    false
}

pub const ROUTES: &[Route] = &[
    route(Method::GET, "/me", "account.me"),
    route(Method::GET, "/system/version", "system.version"),
    route(Method::GET, "/system/status", "system.status"),
    route(Method::GET, "/api-keys", "api_keys.list"),
    route(Method::PATCH, "/api-keys/{id}", "api_keys.update"),
    route(Method::DELETE, "/api-keys/{id}", "api_keys.revoke"),
    route(Method::GET, "/users", "users.list"),
    route(Method::GET, "/audit", "audit.list"),
    route(Method::GET, "/audit/verify", "audit.verify"),
    route(Method::GET, "/projects", "projects.list"),
    route(Method::POST, "/projects", "projects.create"),
    route(Method::GET, "/projects/{id}", "projects.get"),
    route(Method::PATCH, "/projects/{id}", "projects.update"),
    route(Method::POST, "/projects/{projectId}/keys", "projects.create_key"),
    route(Method::DELETE, "/keys/{id}", "projects.revoke_key"),
    route(Method::GET, "/sessions", "sessions.list"),
    route(Method::GET, "/sessions/{id}", "sessions.get"),
    route(Method::GET, "/sessions/{id}/timeline", "sessions.timeline"),
    route(Method::DELETE, "/sessions/{id}", "sessions.delete"),
    route(Method::GET, "/requests/{id}/body", "sessions.request_body"),
    route(Method::GET, "/events", "events.search"),
    route(Method::GET, "/events/groups", "events.group"),
];

fn filter(method: &Method) -> MethodFilter {
    match *method {
        Method::POST => MethodFilter::POST,
        Method::PUT => MethodFilter::PUT,
        Method::PATCH => MethodFilter::PATCH,
        Method::DELETE => MethodFilter::DELETE,
        _ => MethodFilter::GET,
    }
}

/// The type a property takes in a service's input schema, so query strings can be typed.
fn property_type(schema: &Value, name: &str) -> Option<&'static str> {
    let types = &schema["properties"][name]["type"];
    let names: Vec<&str> = match types {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    ["boolean", "integer", "number"].into_iter().find(|t| names.contains(t))
}

fn typed(schema: &Value, name: &str, raw: &str) -> Result<Value, AppError> {
    let invalid = || bad_request(format!("{name} is not valid"));
    Ok(match property_type(schema, name) {
        Some("boolean") => Value::Bool(raw.parse().map_err(|_| invalid())?),
        Some("integer") => Value::from(raw.parse::<i64>().map_err(|_| invalid())?),
        Some("number") => Value::from(raw.parse::<f64>().map_err(|_| invalid())?),
        _ => Value::String(raw.to_string()),
    })
}

pub fn query_pairs(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}

pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'+', _) => out.push(b' '),
            (b'%', Some(b)) => {
                out.push(b);
                i += 2;
            }
            (b, _) => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn input_for(
    state: &AppState,
    route: &Route,
    params: HashMap<String, String>,
    query: Option<String>,
    body: &[u8],
) -> Result<Value, AppError> {
    let service = &state.services[route.service];
    let schema = serde_json::to_value(service.input_schema()).unwrap_or_default();
    let mut input = if body.iter().all(u8::is_ascii_whitespace) {
        Map::new()
    } else {
        match serde_json::from_slice(body) {
            Ok(Value::Object(map)) => map,
            _ => return Err(bad_request("The body must be a JSON object")),
        }
    };
    if let Some(query) = query {
        for (key, value) in query_pairs(&query) {
            let value = typed(&schema, &key, &value)?;
            input.insert(key, value);
        }
    }
    for (key, value) in params {
        let value = typed(&schema, &key, &value)?;
        input.insert(key, value);
    }
    for (key, value) in route.fixed {
        input.insert((*key).to_string(), Value::Bool(*value));
    }
    Ok(Value::Object(input))
}

enum Replay {
    Fresh,
    Stored(Response),
}

fn actor_key(ctx: &Ctx) -> Option<String> {
    let actor = ctx.actor.as_ref()?;
    Some(match actor.credential {
        Credential::ApiKey(id) => format!("key:{id}"),
        _ => format!("user:{}", actor.user_id),
    })
}

/// Stored responses can hold API keys, so they are encrypted.
fn response_context(actor: &str, key: &str) -> String {
    format!("idempotency:{actor}:{key}")
}

async fn claim(ctx: &Ctx, actor: &str, key: &str, fingerprint: &[u8]) -> Result<Replay, AppError> {
    if key.is_empty() || key.len() > 200 {
        return Err(bad_request("Idempotency-Key is 1 to 200 characters"));
    }
    let mut conn = ctx.db().acquire().await?;
    let inserted = sqlx::query("insert into idempotency_keys (actor, key, request_sha256) values ($1, $2, $3) on conflict do nothing")
        .bind(actor)
        .bind(key)
        .bind(fingerprint)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if inserted == 1 {
        return Ok(Replay::Fresh);
    }
    let (stored_hash, status, response): (Vec<u8>, Option<i32>, Option<Value>) =
        sqlx::query_as("select request_sha256, status, response from idempotency_keys where actor = $1 and key = $2")
            .bind(actor)
            .bind(key)
            .fetch_one(&mut *conn)
            .await?;
    if stored_hash != fingerprint {
        return Err(AppError::new(ErrorKind::Conflict, "This Idempotency-Key was already used for a different request"));
    }
    match (status, response) {
        (Some(status), Some(Value::String(sealed))) => {
            let plain = ctx
                .keys()
                .decrypt_str(KeyPurpose::Settings, &response_context(actor, key), &sealed)
                .map_err(|_| AppError::new(ErrorKind::Conflict, "This Idempotency-Key cannot be replayed"))?;
            let body: Value = serde_json::from_str(&plain).map_err(AppError::internal)?;
            let status = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
            let mut response = (status, Json(body)).into_response();
            response.headers_mut().insert("idempotent-replayed", header::HeaderValue::from_static("true"));
            Ok(Replay::Stored(response))
        }
        (Some(_), Some(_)) => Err(AppError::new(ErrorKind::Conflict, "This Idempotency-Key cannot be replayed")),
        _ => Err(AppError::new(ErrorKind::Conflict, "A request with this Idempotency-Key is still running")),
    }
}

async fn settle(ctx: &Ctx, actor: &str, key: &str, outcome: Option<(StatusCode, &Value)>) {
    let Ok(mut conn) = ctx.db().acquire().await else { return };
    let result = match outcome {
        Some((status, body)) => {
            sqlx::query("update idempotency_keys set status = $3, response = $4 where actor = $1 and key = $2")
                .bind(actor)
                .bind(key)
                .bind(i32::from(status.as_u16()))
                .bind(Value::String(ctx.keys().encrypt_str(KeyPurpose::Settings, &response_context(actor, key), &body.to_string())))
                .execute(&mut *conn)
                .await
        }
        None => sqlx::query("delete from idempotency_keys where actor = $1 and key = $2").bind(actor).bind(key).execute(&mut *conn).await,
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, "could not record an idempotent response");
    }
}

fn status_of(e: &AppError) -> StatusCode {
    StatusCode::from_u16(e.kind.status()).unwrap_or(StatusCode::BAD_REQUEST)
}

async fn handle(
    state: AppState,
    route: &'static Route,
    params: HashMap<String, String>,
    query: Option<String>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let request_id = parts.extensions.get::<RequestId>().map(|r| r.0.clone()).unwrap_or_default();
    let fail = |e: AppError| ApiError(e, Some(request_id.clone())).into_response();
    let peer = parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0);
    let ip = security::client_ip(&state, &parts.headers, peer).await;
    let ctx = match auth::context(&state, &parts.headers, ip.map(|i| i.to_string()), Surface::Api, request_id.clone()).await {
        Ok(ctx) => ctx,
        Err(e) => return fail(e),
    };
    let Some(service) = state.services.get(route.service) else { return fail(AppError::new(ErrorKind::NotFound, "Not found")) };
    if ctx.actor.as_ref().is_some_and(|a| a.credential != Credential::Session) && !service.meta().is_tool() {
        return fail(AppError::new(ErrorKind::Forbidden, "This call is only available signed in to the app"));
    }
    if let Err(e) = opentrail_core::service::authorize(service.meta(), &ctx) {
        return fail(e);
    }
    let Ok(body) = to_bytes(body, crate::rpc::body_limit(route.service)).await else {
        return json_error(StatusCode::PAYLOAD_TOO_LARGE, "bad_request", "The request is too large");
    };
    let input = match input_for(&state, route, params, query, &body) {
        Ok(input) => input,
        Err(e) => return fail(e),
    };

    let idempotency = match (parts.headers.get("idempotency-key"), actor_key(&ctx)) {
        (Some(key), Some(actor)) if route.method != Method::GET => {
            let key = key.to_str().unwrap_or_default().to_string();
            let fingerprint = sha256_hex(format!("{} {} {}", route.method, parts.uri.path(), input)).into_bytes();
            match claim(&ctx, &actor, &key, &fingerprint).await {
                Ok(Replay::Fresh) => Some((actor, key)),
                Ok(Replay::Stored(response)) => return response,
                Err(e) => return fail(e),
            }
        }
        _ => None,
    };

    let result = service.invoke(&ctx, input).await;
    let status = if route.creates { StatusCode::CREATED } else { StatusCode::OK };
    if let Some((actor, key)) = &idempotency {
        match &result {
            Ok(body) => settle(&ctx, actor, key, Some((status, body))).await,
            Err(e) if e.kind == ErrorKind::Internal => settle(&ctx, actor, key, None).await,
            Err(e) => {
                let body = json!({ "error": e.message, "kind": e.kind });
                settle(&ctx, actor, key, Some((status_of(e), &body))).await;
            }
        }
    }
    match result {
        Ok(output) => {
            let mut response = (status, Json(output)).into_response();
            response.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
            response
        }
        Err(e) => fail(e),
    }
}

async fn openapi(State(state): State<AppState>) -> Response {
    let base = state.app.env.app_url.clone().unwrap_or_default();
    Json(crate::openapi::document(&state, &format!("{}/api/v1", base.trim_end_matches('/')))).into_response()
}

pub fn router(state: &AppState) -> Router<AppState> {
    let mut router = Router::new().route("/openapi.json", get(openapi));
    let mut by_path: HashMap<&str, Vec<&'static Route>> = HashMap::new();
    for r in ROUTES {
        by_path.entry(r.path).or_default().push(r);
    }
    for (path, routes) in by_path {
        let mut method_router = axum::routing::MethodRouter::new();
        for r in routes {
            debug_assert!(state.services.contains_key(r.service), "{} is not registered", r.service);
            method_router = method_router.merge(on(
                filter(&r.method),
                move |State(state): State<AppState>,
                      params: Option<Path<HashMap<String, String>>>,
                      RawQuery(query): RawQuery,
                      request: Request| { handle(state, r, params.map(|p| p.0).unwrap_or_default(), query, request) },
            ));
        }
        router = router.route(path, method_router);
    }
    router
}
