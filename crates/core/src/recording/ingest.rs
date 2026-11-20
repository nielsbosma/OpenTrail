//! The one write path the public can reach. The caller holds only a public key, so everything is
//! bounded: body size, decompressed size, events, timestamps, session length and size, and the
//! project's monthly quota. There is no actor and no audit entry: recordings are data, not changes.

use std::net::IpAddr;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::extract::{Extracted, extract, ts_of};
use super::mask::sanitize;
use super::{INGEST_KEY_PREFIX, browser_and_os, gunzip, ip_prefix, project_key, seal_body, seal_chunk};
use crate::context::App;
use crate::error::AppError;

/// Compressed, as sent.
pub const MAX_BODY: usize = 512 * 1024;
pub const MAX_JSON: u64 = 8 * 1024 * 1024;
pub const MAX_EVENTS: usize = 20_000;
pub const MAX_SESSION_BYTES: i64 = 100 * 1024 * 1024;
pub const MAX_SESSION_HOURS: i64 = 4;

pub struct Request<'a> {
    pub key: &'a str,
    pub origin: Option<&'a str>,
    pub ip: Option<IpAddr>,
    pub user_agent: Option<&'a str>,
    pub body: &'a [u8],
    pub now: DateTime<Utc>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Stored,
    /// The same chunk arrived before, so a retry changes nothing.
    Duplicate,
    /// The session is over its length or size, so the batch was dropped and the session marked.
    Truncated,
}

#[derive(Debug)]
pub enum Refusal {
    Invalid(String),
    UnknownKey,
    Origin,
    Archived,
    TooLarge,
    Quota,
    OtherProject,
}

#[derive(Debug)]
pub enum IngestError {
    Refused(Refusal),
    Internal(AppError),
}

impl From<AppError> for IngestError {
    fn from(e: AppError) -> Self {
        IngestError::Internal(e)
    }
}

impl From<sqlx::Error> for IngestError {
    fn from(e: sqlx::Error) -> Self {
        IngestError::Internal(e.into())
    }
}

fn refuse<T>(r: Refusal) -> Result<T, IngestError> {
    Err(IngestError::Refused(r))
}

fn invalid<T>(message: &str) -> Result<T, IngestError> {
    refuse(Refusal::Invalid(message.into()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Batch {
    v: u8,
    session_id: Uuid,
    window_id: Uuid,
    seq: i32,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    lang: Option<String>,
    events: Vec<Value>,
}

#[derive(sqlx::FromRow)]
struct KeyRow {
    key_id: Uuid,
    project_id: Uuid,
    allowed_origins: Vec<String>,
    capture_bodies: bool,
    capture_canvas: bool,
    monthly_session_quota: i32,
    data_key: String,
    archived: bool,
    last_used_at: Option<DateTime<Utc>>,
}

/// The origins a key may send from, for answering a CORS preflight. `None` for an unknown key.
pub async fn allowed_origins(app: &App, key: &str) -> Result<Option<Vec<String>>, IngestError> {
    if !plausible_key(key) {
        return Ok(None);
    }
    Ok(sqlx::query_scalar(
        "select p.allowed_origins from ingest_keys k join projects p on p.id = k.project_id
          where k.key = $1 and k.revoked_at is null and p.archived_at is null",
    )
    .bind(key)
    .fetch_optional(&app.db)
    .await?)
}

fn plausible_key(key: &str) -> bool {
    key.starts_with(INGEST_KEY_PREFIX) && key.len() <= 64 && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub async fn ingest(app: &App, request: Request<'_>) -> Result<Outcome, IngestError> {
    if !plausible_key(request.key) {
        return refuse(Refusal::UnknownKey);
    }
    if request.body.len() > MAX_BODY {
        return refuse(Refusal::TooLarge);
    }
    let json = if request.body.starts_with(&[0x1f, 0x8b]) {
        match gunzip(request.body, MAX_JSON) {
            Some(json) => json,
            None => return refuse(Refusal::TooLarge),
        }
    } else {
        request.body.to_vec()
    };
    let Ok(mut batch) = serde_json::from_slice::<Batch>(&json) else { return invalid("The batch is not valid JSON in the expected shape") };
    drop(json);
    if batch.v != 1 {
        return invalid("Unsupported batch version");
    }
    if !(0..=1_000_000).contains(&batch.seq) || batch.events.is_empty() || batch.events.len() > MAX_EVENTS {
        return invalid("The batch's sequence number or event count is out of range");
    }
    let mut first = DateTime::<Utc>::MAX_UTC;
    let mut last = DateTime::<Utc>::MIN_UTC;
    for event in &batch.events {
        let Some(ts) = ts_of(event) else { return invalid("Every event needs a timestamp") };
        if event.get("type").and_then(Value::as_i64).is_none() {
            return invalid("Every event needs a type");
        }
        first = first.min(ts);
        last = last.max(ts);
    }
    if first < request.now - Duration::hours(24) || last > request.now + Duration::minutes(10) {
        return invalid("Event timestamps are too far from the server's clock");
    }

    let mut conn = app.db.acquire().await?;
    let Some(key) = sqlx::query_as::<_, KeyRow>(
        "select k.id as key_id, p.id as project_id, p.allowed_origins, p.capture_bodies, p.capture_canvas, p.monthly_session_quota, p.data_key,
                p.archived_at is not null as archived, k.last_used_at
           from ingest_keys k join projects p on p.id = k.project_id
          where k.key = $1 and k.revoked_at is null",
    )
    .bind(request.key)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return refuse(Refusal::UnknownKey);
    };
    if !request.origin.is_some_and(|o| key.allowed_origins.iter().any(|a| a.eq_ignore_ascii_case(o))) {
        return refuse(Refusal::Origin);
    }
    if key.archived {
        return refuse(Refusal::Archived);
    }
    let data_key = project_key(&app.keys, key.project_id, &key.data_key)?;

    if !key.capture_canvas {
        batch.events.retain(|e| !is_canvas(e));
        if batch.events.is_empty() {
            return Ok(Outcome::Stored);
        }
    }
    for event in &mut batch.events {
        sanitize(event);
    }

    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let session: Option<(Uuid, DateTime<Utc>, i64, bool)> =
        sqlx::query_as("select project_id, started_at, bytes, truncated from recorded_sessions where id = $1 for update")
            .bind(batch.session_id)
            .fetch_optional(&mut *tx)
            .await?;
    let (started_at, bytes) = match session {
        Some((project_id, _, _, _)) if project_id != key.project_id => return refuse(Refusal::OtherProject),
        Some((_, _, _, true)) => return Ok(Outcome::Truncated),
        Some((_, started_at, bytes, false)) => (started_at, bytes),
        None => {
            let this_month: i64 = sqlx::query_scalar(
                "select count(*) from recorded_sessions where project_id = $1 and started_at >= date_trunc('month', $2::timestamptz at time zone 'UTC') at time zone 'UTC'",
            )
            .bind(key.project_id)
            .bind(request.now)
            .fetch_one(&mut *tx)
            .await?;
            if this_month >= i64::from(key.monthly_session_quota) {
                return refuse(Refusal::Quota);
            }
            create_session(&mut tx, &key, &batch, first, &request).await?;
            (first, 0)
        }
    };

    let previous_url: Option<String> =
        sqlx::query_scalar("select url from pageviews where session_id = $1 and window_id = $2 order by ts desc limit 1")
            .bind(batch.session_id)
            .bind(batch.window_id)
            .fetch_optional(&mut *tx)
            .await?;
    let rows = extract(&mut batch.events, previous_url);
    let events_json = serde_json::to_vec(&batch.events).map_err(AppError::internal)?;
    let sealed = seal_chunk(&data_key, batch.session_id, batch.window_id, batch.seq, &events_json);

    if last - started_at > Duration::hours(MAX_SESSION_HOURS) || bytes + sealed.len() as i64 > MAX_SESSION_BYTES {
        sqlx::query("update recorded_sessions set truncated = true where id = $1").bind(batch.session_id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(Outcome::Truncated);
    }

    let inserted = sqlx::query(
        "insert into replay_chunks (session_id, window_id, seq, first_ts, last_ts, event_count, data) values ($1, $2, $3, $4, $5, $6, $7)
         on conflict do nothing",
    )
    .bind(batch.session_id)
    .bind(batch.window_id)
    .bind(batch.seq)
    .bind(first)
    .bind(last)
    .bind(batch.events.len() as i32)
    .bind(&sealed)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Ok(Outcome::Duplicate);
    }

    let ids = Ids { session: batch.session_id, project: key.project_id, window: batch.window_id };
    store_rows(&mut tx, &ids, &rows, key.capture_bodies.then_some(&data_key)).await?;

    let failed = rows.network.iter().filter(|n| n.status == 0 || n.status >= 400).count() as i32;
    sqlx::query(
        "update recorded_sessions set last_event_at = greatest(last_event_at, $2), chunks = chunks + 1, bytes = bytes + $3,
                event_count = event_count + $4, page_count = page_count + $5, error_count = error_count + $6,
                failed_request_count = failed_request_count + $7, user_ref = coalesce($8, user_ref),
                first_url = coalesce(first_url, $9),
                screen_width = coalesce(screen_width, $10), screen_height = coalesce(screen_height, $11),
                windows = case when $12 then (select count(distinct window_id) from replay_chunks where session_id = $1)::int else windows end
          where id = $1",
    )
    .bind(batch.session_id)
    .bind(last)
    .bind(sealed.len() as i64)
    .bind(batch.events.len() as i64)
    .bind(rows.pages.len() as i32)
    .bind(rows.errors.len() as i32)
    .bind(failed)
    .bind(&rows.user_ref)
    .bind(rows.pages.first().map(|p| &p.url))
    .bind(rows.screen.map(|s| s.0))
    .bind(rows.screen.map(|s| s.1))
    .bind(batch.seq == 0)
    .execute(&mut *tx)
    .await?;
    if key.last_used_at.is_none_or(|at| at < request.now - Duration::minutes(1)) {
        sqlx::query("update ingest_keys set last_used_at = $2 where id = $1").bind(key.key_id).bind(request.now).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Outcome::Stored)
}

/// rrweb's canvas mutations (incremental source 9), large and only kept when the project asks.
fn is_canvas(event: &Value) -> bool {
    event.get("type").and_then(Value::as_i64) == Some(3) && event["data"].get("source").and_then(Value::as_i64) == Some(9)
}

async fn create_session(
    conn: &mut PgConnection,
    key: &KeyRow,
    batch: &Batch,
    first: DateTime<Utc>,
    request: &Request<'_>,
) -> Result<(), IngestError> {
    let (browser, os) = request.user_agent.map(browser_and_os).unwrap_or_default();
    let short = |s: &Option<String>, max: usize| {
        s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(max).collect::<String>())
    };
    let inserted = sqlx::query(
        "insert into recorded_sessions (id, project_id, started_at, last_event_at, release, browser, os, lang, ip_prefix)
         values ($1, $2, $3, $3, $4, $5, $6, $7, $8) on conflict (id) do nothing",
    )
    .bind(batch.session_id)
    .bind(key.project_id)
    .bind(first)
    .bind(short(&batch.release, 100))
    .bind(browser)
    .bind(os)
    .bind(short(&batch.lang, 35))
    .bind(request.ip.map(ip_prefix))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if inserted == 0 {
        // Another tab created it a moment ago. Make sure it is ours.
        let project: Uuid = sqlx::query_scalar("select project_id from recorded_sessions where id = $1")
            .bind(batch.session_id)
            .fetch_one(&mut *conn)
            .await?;
        if project != key.project_id {
            return refuse(Refusal::OtherProject);
        }
    }
    Ok(())
}

struct Ids {
    session: Uuid,
    project: Uuid,
    window: Uuid,
}

async fn store_rows(
    conn: &mut PgConnection,
    ids: &Ids,
    x: &Extracted,
    bodies: Option<&opentrail_crypto::DataKey>,
) -> Result<(), IngestError> {
    if !x.pages.is_empty() {
        sqlx::query(
            "insert into pageviews (session_id, project_id, window_id, ts, url, title)
             select $1, $2, $3, * from unnest($4::timestamptz[], $5::text[], $6::text[])",
        )
        .bind(ids.session)
        .bind(ids.project)
        .bind(ids.window)
        .bind(x.pages.iter().map(|p| p.ts).collect::<Vec<_>>())
        .bind(x.pages.iter().map(|p| p.url.clone()).collect::<Vec<_>>())
        .bind(x.pages.iter().map(|p| p.title.clone()).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?;
    }
    if !x.elements.is_empty() {
        let e: Vec<_> = x.elements.iter().collect();
        sqlx::query(
            "insert into elements (session_id, window_id, node_id, role, name, testid, component)
             select $1, $2, * from unnest($3::int[], $4::text[], $5::text[], $6::text[], $7::text[])
             on conflict (session_id, window_id, node_id) do update
               set role = excluded.role, name = excluded.name, testid = excluded.testid, component = excluded.component",
        )
        .bind(ids.session)
        .bind(ids.window)
        .bind(e.iter().map(|(id, _)| **id).collect::<Vec<_>>())
        .bind(e.iter().map(|(_, el)| el.role.clone()).collect::<Vec<_>>())
        .bind(e.iter().map(|(_, el)| el.name.clone()).collect::<Vec<_>>())
        .bind(e.iter().map(|(_, el)| el.testid.clone()).collect::<Vec<_>>())
        .bind(e.iter().map(|(_, el)| el.component.clone()).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?;
    }
    if !x.interactions.is_empty() {
        let i = &x.interactions;
        sqlx::query(
            "insert into interactions (session_id, project_id, window_id, ts, url, kind, node_id, role, name, testid, component, x, y, input_length)
             select $1, $2, $3, u.ts, u.url, u.kind, u.node_id, e.role, e.name, e.testid, e.component, u.x, u.y, u.len
               from unnest($4::timestamptz[], $5::text[], $6::text[], $7::int[], $8::int[], $9::int[], $10::int[]) as u(ts, url, kind, node_id, x, y, len)
               left join elements e on e.session_id = $1 and e.window_id = $3 and e.node_id = u.node_id",
        )
        .bind(ids.session)
        .bind(ids.project)
        .bind(ids.window)
        .bind(i.iter().map(|r| r.ts).collect::<Vec<_>>())
        .bind(i.iter().map(|r| r.url.clone()).collect::<Vec<_>>())
        .bind(i.iter().map(|r| r.kind.to_string()).collect::<Vec<_>>())
        .bind(i.iter().map(|r| r.node_id).collect::<Vec<_>>())
        .bind(i.iter().map(|r| r.x).collect::<Vec<_>>())
        .bind(i.iter().map(|r| r.y).collect::<Vec<_>>())
        .bind(i.iter().map(|r| r.input_length).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?;
    }
    if !x.console.is_empty() {
        let c = &x.console;
        sqlx::query(
            "insert into console_logs (session_id, project_id, window_id, ts, level, message, stack)
             select $1, $2, $3, * from unnest($4::timestamptz[], $5::text[], $6::text[], $7::text[])",
        )
        .bind(ids.session)
        .bind(ids.project)
        .bind(ids.window)
        .bind(c.iter().map(|r| r.ts).collect::<Vec<_>>())
        .bind(c.iter().map(|r| r.level.to_string()).collect::<Vec<_>>())
        .bind(c.iter().map(|r| r.message.clone()).collect::<Vec<_>>())
        .bind(c.iter().map(|r| r.stack.clone()).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?;
    }
    if !x.network.is_empty() {
        let n = &x.network;
        let seal = |ts, which, body: &Option<String>| match (bodies, body) {
            (Some(key), Some(body)) => Some(seal_body(key, ids.session, ts, which, body)),
            _ => None,
        };
        sqlx::query(
            "insert into network_requests (session_id, project_id, window_id, ts, initiator, method, url, status, duration_ms, bytes, request_body, response_body)
             select $1, $2, $3, * from unnest($4::timestamptz[], $5::text[], $6::text[], $7::text[], $8::int[], $9::int[], $10::bigint[], $11::bytea[], $12::bytea[])",
        )
        .bind(ids.session)
        .bind(ids.project)
        .bind(ids.window)
        .bind(n.iter().map(|r| r.ts).collect::<Vec<_>>())
        .bind(n.iter().map(|r| r.initiator.to_string()).collect::<Vec<_>>())
        .bind(n.iter().map(|r| r.method.clone()).collect::<Vec<_>>())
        .bind(n.iter().map(|r| r.url.clone()).collect::<Vec<_>>())
        .bind(n.iter().map(|r| r.status).collect::<Vec<_>>())
        .bind(n.iter().map(|r| r.duration_ms).collect::<Vec<_>>())
        .bind(n.iter().map(|r| r.bytes).collect::<Vec<_>>())
        .bind(n.iter().map(|r| seal(r.ts, "request", &r.request_body)).collect::<Vec<_>>())
        .bind(n.iter().map(|r| seal(r.ts, "response", &r.response_body)).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?;
    }
    if !x.errors.is_empty() {
        let e = &x.errors;
        sqlx::query(
            "insert into errors (session_id, project_id, window_id, ts, kind, message, stack, url)
             select $1, $2, $3, * from unnest($4::timestamptz[], $5::text[], $6::text[], $7::text[], $8::text[])",
        )
        .bind(ids.session)
        .bind(ids.project)
        .bind(ids.window)
        .bind(e.iter().map(|r| r.ts).collect::<Vec<_>>())
        .bind(e.iter().map(|r| r.kind.to_string()).collect::<Vec<_>>())
        .bind(e.iter().map(|r| r.message.clone()).collect::<Vec<_>>())
        .bind(e.iter().map(|r| r.stack.clone()).collect::<Vec<_>>())
        .bind(e.iter().map(|r| r.url.clone()).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?;
    }
    if !x.vitals.is_empty() {
        let v = &x.vitals;
        sqlx::query(
            "insert into vitals (session_id, project_id, window_id, ts, url, name, value)
             select $1, $2, $3, * from unnest($4::timestamptz[], $5::text[], $6::text[], $7::float8[])",
        )
        .bind(ids.session)
        .bind(ids.project)
        .bind(ids.window)
        .bind(v.iter().map(|r| r.ts).collect::<Vec<_>>())
        .bind(v.iter().map(|r| r.url.clone()).collect::<Vec<_>>())
        .bind(v.iter().map(|r| r.name.to_string()).collect::<Vec<_>>())
        .bind(v.iter().map(|r| r.value).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?;
    }
    if !x.custom.is_empty() {
        let c = &x.custom;
        sqlx::query(
            "insert into custom_events (session_id, project_id, window_id, ts, name, props)
             select $1, $2, $3, * from unnest($4::timestamptz[], $5::text[], $6::jsonb[])",
        )
        .bind(ids.session)
        .bind(ids.project)
        .bind(ids.window)
        .bind(c.iter().map(|r| r.ts).collect::<Vec<_>>())
        .bind(c.iter().map(|r| r.name.clone()).collect::<Vec<_>>())
        .bind(c.iter().map(|r| r.props.clone()).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
