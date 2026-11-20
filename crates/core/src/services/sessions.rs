use async_trait::async_trait;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::audit::{Entry, audit};
use crate::context::Ctx;
use crate::error::{AppError, Result, bad_request, not_found};
use crate::permissions::Permission;
use crate::recording::{open_body, open_chunk, project_key};
use crate::service::{Access, Meta, Service};

#[derive(Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_name: String,
    pub started_at: DateTime<Utc>,
    pub last_event_at: DateTime<Utc>,
    pub duration_seconds: i64,
    pub first_url: Option<String>,
    /// The id the site passed to `identify()`, if it did.
    pub user_ref: Option<String>,
    pub release: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub screen_width: Option<i32>,
    pub screen_height: Option<i32>,
    pub lang: Option<String>,
    pub ip_prefix: Option<String>,
    pub windows: i32,
    pub page_count: i32,
    pub error_count: i32,
    pub failed_request_count: i32,
    pub event_count: i64,
    /// Stored size, compressed and encrypted.
    pub bytes: i64,
    /// The recording stopped early: the session ran over 4 hours or 100 MB.
    pub truncated: bool,
}

const SUMMARY: &str = "select s.id, s.project_id, p.name as project_name, s.started_at, s.last_event_at,
        extract(epoch from s.last_event_at - s.started_at)::bigint as duration_seconds, s.first_url, s.user_ref, s.release,
        s.browser, s.os, s.screen_width, s.screen_height, s.lang, s.ip_prefix, s.windows, s.page_count, s.error_count,
        s.failed_request_count, s.event_count, s.bytes, s.truncated
   from recorded_sessions s join projects p on p.id = s.project_id";

#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListInput {
    pub project_id: Option<Uuid>,
    /// Sessions that started at or after this time.
    pub from: Option<DateTime<Utc>>,
    /// Sessions that started before this time.
    pub to: Option<DateTime<Utc>>,
    pub user_ref: Option<String>,
    /// Part of the URL of any page the session visited, case-insensitive.
    pub url: Option<String>,
    /// Only sessions with (true) or without (false) JavaScript errors.
    pub has_errors: Option<bool>,
    /// Only sessions with (true) or without (false) requests that failed or answered 400 or more.
    pub has_failed_requests: Option<bool>,
    pub min_duration_seconds: Option<i64>,
    pub browser: Option<String>,
    pub release: Option<String>,
    /// At most 200; 50 if left out.
    pub limit: Option<i64>,
    /// The `next` value of the previous page.
    pub before: Option<DateTime<Utc>>,
}

#[derive(Serialize, JsonSchema)]
pub struct SessionPage {
    pub items: Vec<SessionSummary>,
    /// Pass as `before` for the next, older page. Absent on the last page.
    pub next: Option<DateTime<Utc>>,
}

pub struct List;

#[async_trait]
impl Service for List {
    type Input = ListInput;
    type Output = SessionPage;
    const META: Meta = Meta::query(
        "sessions.list",
        "Find recorded sessions, newest first, by project, time, user, visited URL, browser, release, duration, and whether they had errors or failed requests. Each has its counts; read one with sessions.timeline.",
        Access::Permission(Permission::SessionsRead),
    )
    .tool();

    async fn run(_ctx: &Ctx, conn: &mut PgConnection, input: ListInput) -> Result<SessionPage> {
        let limit = input.limit.unwrap_or(50).clamp(1, 200);
        let items: Vec<SessionSummary> = sqlx::query_as(&format!(
            "{SUMMARY}
              where ($1::uuid is null or s.project_id = $1)
                and ($2::timestamptz is null or s.started_at >= $2)
                and ($3::timestamptz is null or s.started_at < $3)
                and ($4::text is null or s.user_ref = $4)
                and ($5::text is null or exists (select 1 from pageviews v where v.session_id = s.id and strpos(lower(v.url), lower($5)) > 0)
                     or strpos(lower(coalesce(s.first_url, '')), lower($5)) > 0)
                and ($6::bool is null or (s.error_count > 0) = $6)
                and ($7::bool is null or (s.failed_request_count > 0) = $7)
                and ($8::bigint is null or s.last_event_at - s.started_at >= make_interval(secs => $8))
                and ($9::text is null or strpos(lower(coalesce(s.browser, '')), lower($9)) > 0)
                and ($10::text is null or s.release = $10)
                and ($11::timestamptz is null or s.started_at < $11)
              order by s.started_at desc
              limit $12"
        ))
        .bind(input.project_id)
        .bind(input.from)
        .bind(input.to)
        .bind(input.user_ref.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .bind(input.url.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .bind(input.has_errors)
        .bind(input.has_failed_requests)
        .bind(input.min_duration_seconds)
        .bind(input.browser.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .bind(input.release.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .bind(input.before)
        .bind(limit + 1)
        .fetch_all(&mut *conn)
        .await?;
        let mut items = items;
        let next = if items.len() as i64 > limit {
            items.truncate(limit as usize);
            items.last().map(|s| s.started_at)
        } else {
            None
        };
        Ok(SessionPage { items, next })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdInput {
    pub id: Uuid,
}

#[derive(Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PageView {
    pub ts: DateTime<Utc>,
    pub window_id: Uuid,
    pub url: String,
    pub title: Option<String>,
}

#[derive(Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WindowView {
    pub window_id: Uuid,
    pub first_ts: DateTime<Utc>,
    pub last_ts: DateTime<Utc>,
    pub chunks: i64,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    #[serde(flatten)]
    pub session: SessionSummary,
    /// Browser tabs in the session, each a separate recording.
    pub windows: Vec<WindowView>,
    pub pages: Vec<PageView>,
}

async fn summary(conn: &mut PgConnection, id: Uuid) -> Result<SessionSummary> {
    sqlx::query_as(&format!("{SUMMARY} where s.id = $1")).bind(id).fetch_optional(&mut *conn).await?.ok_or_else(|| not_found("Session"))
}

pub struct Get;

#[async_trait]
impl Service for Get {
    type Input = IdInput;
    type Output = SessionDetail;
    const META: Meta = Meta::query(
        "sessions.get",
        "Get one session: browser, user, counts, its tabs and every page it visited in order.",
        Access::Permission(Permission::SessionsRead),
    )
    .tool();

    async fn run(_ctx: &Ctx, conn: &mut PgConnection, input: IdInput) -> Result<SessionDetail> {
        let session = summary(conn, input.id).await?;
        let windows = sqlx::query_as(
            "select window_id, min(first_ts) as first_ts, max(last_ts) as last_ts, count(*) as chunks
               from replay_chunks where session_id = $1 group by window_id order by min(first_ts)",
        )
        .bind(input.id)
        .fetch_all(&mut *conn)
        .await?;
        let pages = sqlx::query_as("select ts, window_id, url, title from pageviews where session_id = $1 order by ts, id limit 2000")
            .bind(input.id)
            .fetch_all(&mut *conn)
            .await?;
        Ok(SessionDetail { session, windows, pages })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Page,
    Interaction,
    Console,
    Network,
    Error,
    Vital,
    Custom,
}

impl EventKind {
    const ALL: [EventKind; 7] = [
        EventKind::Page,
        EventKind::Interaction,
        EventKind::Console,
        EventKind::Network,
        EventKind::Error,
        EventKind::Vital,
        EventKind::Custom,
    ];

    /// The table, a JSON object of its fields, and the column text filters match.
    fn sql(self) -> (&'static str, &'static str, &'static str) {
        match self {
            EventKind::Page => ("pageviews", "jsonb_build_object('url', url, 'title', title)", "url"),
            EventKind::Interaction => (
                "interactions",
                "jsonb_strip_nulls(jsonb_build_object('kind', kind, 'url', url, 'nodeId', node_id, 'role', role, 'name', name, 'testid', testid, 'component', component, 'x', x, 'y', y, 'inputLength', input_length))",
                "coalesce(name, '') || ' ' || coalesce(testid, '') || ' ' || coalesce(component, '')",
            ),
            EventKind::Console => {
                ("console_logs", "jsonb_strip_nulls(jsonb_build_object('level', level, 'message', message, 'stack', stack))", "message")
            }
            EventKind::Network => (
                "network_requests",
                "jsonb_strip_nulls(jsonb_build_object('id', id, 'initiator', initiator, 'method', method, 'url', url, 'status', status, 'durationMs', duration_ms, 'bytes', bytes, 'hasRequestBody', request_body is not null, 'hasResponseBody', response_body is not null))",
                "url",
            ),
            EventKind::Error => {
                ("errors", "jsonb_strip_nulls(jsonb_build_object('kind', kind, 'message', message, 'stack', stack, 'url', url))", "message")
            }
            EventKind::Vital => ("vitals", "jsonb_strip_nulls(jsonb_build_object('name', name, 'value', value, 'url', url))", "name"),
            EventKind::Custom => ("custom_events", "jsonb_strip_nulls(jsonb_build_object('name', name, 'props', props))", "name"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            EventKind::Page => "page",
            EventKind::Interaction => "interaction",
            EventKind::Console => "console",
            EventKind::Network => "network",
            EventKind::Error => "error",
            EventKind::Vital => "vital",
            EventKind::Custom => "custom",
        }
    }
}

#[derive(Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub session_id: Uuid,
    pub window_id: Uuid,
    pub ts: DateTime<Utc>,
    pub kind: String,
    pub data: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelineInput {
    pub id: Uuid,
    /// Only these kinds. All of them if left out.
    pub kinds: Option<Vec<EventKind>>,
    /// Events after this time, for paging.
    pub after: Option<DateTime<Utc>>,
    /// At most 5000; 1000 if left out.
    pub limit: Option<i64>,
}

#[derive(Serialize, JsonSchema)]
pub struct Timeline {
    pub items: Vec<EventRow>,
    /// Pass as `after` for the next page. Absent on the last page.
    pub next: Option<DateTime<Utc>>,
}

pub struct GetTimeline;

#[async_trait]
impl Service for GetTimeline {
    type Input = TimelineInput;
    type Output = Timeline;
    const META: Meta = Meta::query(
        "sessions.timeline",
        "Read what happened in one session, in time order: pages, clicks and inputs (with the element's role, name, test id and component, never what was typed), console output, network requests, JavaScript errors, Web Vitals and custom events. Filter by kind and page with after.",
        Access::Permission(Permission::SessionsRead),
    )
    .tool();

    async fn run(_ctx: &Ctx, conn: &mut PgConnection, input: TimelineInput) -> Result<Timeline> {
        summary(conn, input.id).await?;
        let limit = input.limit.unwrap_or(1000).clamp(1, 5000);
        let kinds = input.kinds.unwrap_or_else(|| EventKind::ALL.to_vec());
        if kinds.is_empty() {
            return Ok(Timeline { items: vec![], next: None });
        }
        let parts: Vec<String> = kinds
            .iter()
            .map(|k| {
                let (table, data, _) = k.sql();
                format!(
                    "select session_id, window_id, ts, '{}' as kind, {data} as data, id::bigint as seq from {table}
                      where session_id = $1 and ($2::timestamptz is null or ts > $2)",
                    k.as_str()
                )
            })
            .collect();
        let sql =
            format!("select session_id, window_id, ts, kind, data from ({}) e order by ts, kind, seq limit $3", parts.join(" union all "));
        let mut items: Vec<EventRow> = sqlx::query_as(&sql).bind(input.id).bind(input.after).bind(limit + 1).fetch_all(&mut *conn).await?;
        let next = page_end(&mut items, limit);
        Ok(Timeline { items, next })
    }
}

/// Trims to `limit` and says where the next page starts. Events sharing the last timestamp are
/// kept together, since the next page starts after it.
fn page_end(items: &mut Vec<EventRow>, limit: i64) -> Option<DateTime<Utc>> {
    if items.len() as i64 <= limit {
        return None;
    }
    let cut = items[limit as usize].ts;
    let keep = items.iter().take(limit as usize).rposition(|e| e.ts < cut).map(|i| i + 1).unwrap_or(limit as usize);
    items.truncate(keep);
    items.last().map(|e| e.ts)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchInput {
    pub kind: EventKind,
    pub project_id: Option<Uuid>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    /// Part of the main text, case-insensitive: the URL of pages and requests, the message of
    /// console output and errors, the element's name, test id or component for interactions, the
    /// name of vitals and custom events.
    pub text: Option<String>,
    /// Console level: log, info, warn, error or debug.
    pub level: Option<String>,
    /// Network requests with at least this status. 400 finds failures; failed requests without a
    /// response have status 0 and are found with `failed`.
    pub min_status: Option<i32>,
    /// Network requests that failed without a response or answered 400 or more.
    pub failed: Option<bool>,
    /// At most 1000; 100 if left out.
    pub limit: Option<i64>,
    /// The `next` value of the previous page, for older events.
    pub before: Option<DateTime<Utc>>,
}

#[derive(Serialize, JsonSchema)]
pub struct SearchPage {
    pub items: Vec<EventRow>,
    /// Pass as `before` for the next, older page.
    pub next: Option<DateTime<Utc>>,
}

fn search_filters(kind: EventKind) -> (&'static str, &'static str) {
    let level = if kind == EventKind::Console { "($7::text is null or level = $7)" } else { "$7::text is null" };
    let network = if kind == EventKind::Network {
        "($8::int is null or status >= $8) and ($9::bool is null or (status = 0 or status >= 400) = $9)"
    } else {
        "$8::int is null and $9::bool is null"
    };
    (level, network)
}

pub struct Search;

#[async_trait]
impl Service for Search {
    type Input = SearchInput;
    type Output = SearchPage;
    const META: Meta = Meta::query(
        "events.search",
        "Find one kind of event across sessions, newest first: for example every JavaScript error with a message, every failed request to a URL, or every click on a button. Each row names its session, which sessions.timeline then shows in context.",
        Access::Permission(Permission::SessionsRead),
    )
    .tool();

    async fn run(_ctx: &Ctx, conn: &mut PgConnection, input: SearchInput) -> Result<SearchPage> {
        let limit = input.limit.unwrap_or(100).clamp(1, 1000);
        let (table, data, text) = input.kind.sql();
        let (level, network) = search_filters(input.kind);
        let sql = format!(
            "select session_id, window_id, ts, '{}' as kind, {data} as data from {table}
              where ($1::uuid is null or project_id = $1) and ($2::timestamptz is null or ts >= $2) and ($3::timestamptz is null or ts < $3)
                and ($4::text is null or strpos(lower({text}), lower($4)) > 0) and ($5::timestamptz is null or ts < $5)
                and {level} and {network}
              order by ts desc limit $6",
            input.kind.as_str()
        );
        let mut items: Vec<EventRow> = sqlx::query_as(&sql)
            .bind(input.project_id)
            .bind(input.from)
            .bind(input.to)
            .bind(input.text.as_deref().map(str::trim).filter(|s| !s.is_empty()))
            .bind(input.before)
            .bind(limit + 1)
            .bind(input.level.as_deref())
            .bind(input.min_status)
            .bind(input.failed)
            .fetch_all(&mut *conn)
            .await?;
        let next = if items.len() as i64 > limit {
            items.truncate(limit as usize);
            items.last().map(|e| e.ts)
        } else {
            None
        };
        Ok(SearchPage { items, next })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    /// Console and error messages.
    Message,
    /// Page and request URLs without their query string.
    Path,
    /// Network status codes.
    Status,
    /// Vital, custom event or element names.
    Name,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupInput {
    pub kind: EventKind,
    pub by: GroupBy,
    pub project_id: Option<Uuid>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    /// The same text filter as events.search.
    pub text: Option<String>,
    /// At most 500; 50 if left out.
    pub limit: Option<i64>,
}

#[derive(Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventGroup {
    pub value: Option<String>,
    pub count: i64,
    pub sessions: i64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// One session it happened in, the latest.
    pub example_session_id: Uuid,
}

pub struct GroupEvents;

#[async_trait]
impl Service for GroupEvents {
    type Input = GroupInput;
    type Output = Vec<EventGroup>;
    const META: Meta = Meta::query(
        "events.group",
        "Count one kind of event across sessions, grouped by message, URL path, status or name: how often each happened, in how many sessions, when first and last, and one example session. Use it to see which errors or failing endpoints are most common.",
        Access::Permission(Permission::SessionsRead),
    )
    .tool();

    async fn run(_ctx: &Ctx, conn: &mut PgConnection, input: GroupInput) -> Result<Vec<EventGroup>> {
        let limit = input.limit.unwrap_or(50).clamp(1, 500);
        let (table, _, text) = input.kind.sql();
        let column = match (input.kind, input.by) {
            (EventKind::Console | EventKind::Error, GroupBy::Message) => "left(message, 300)",
            (EventKind::Page | EventKind::Network, GroupBy::Path) => "split_part(url, '?', 1)",
            (EventKind::Network, GroupBy::Status) => "status::text",
            (EventKind::Vital | EventKind::Custom, GroupBy::Name) => "name",
            (EventKind::Interaction, GroupBy::Name) => "coalesce(name, testid, component)",
            _ => return Err(bad_request("That grouping does not apply to this kind of event")),
        };
        let sql = format!(
            "select {column} as value, count(*) as count, count(distinct session_id) as sessions, min(ts) as first_seen, max(ts) as last_seen,
                    (array_agg(session_id order by ts desc))[1] as example_session_id
               from {table}
              where ($1::uuid is null or project_id = $1) and ($2::timestamptz is null or ts >= $2) and ($3::timestamptz is null or ts < $3)
                and ($4::text is null or strpos(lower({text}), lower($4)) > 0)
              group by 1 order by count(*) desc limit $5"
        );
        Ok(sqlx::query_as(&sql)
            .bind(input.project_id)
            .bind(input.from)
            .bind(input.to)
            .bind(input.text.as_deref().map(str::trim).filter(|s| !s.is_empty()))
            .bind(limit)
            .fetch_all(&mut *conn)
            .await?)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplayInput {
    pub id: Uuid,
    pub window_id: Uuid,
    /// Chunks after this sequence number, for paging.
    pub after_seq: Option<i32>,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Replay {
    /// rrweb events, ready for the rrweb player.
    pub events: Vec<Value>,
    /// Pass as `afterSeq` for the rest. Absent when this was the end.
    pub next_seq: Option<i32>,
}

/// About this much JSON per call, so a long session loads in steps.
const REPLAY_PAGE_BYTES: usize = 8 * 1024 * 1024;

pub struct GetReplay;

#[async_trait]
impl Service for GetReplay {
    type Input = ReplayInput;
    type Output = Replay;
    const META: Meta = Meta::query(
        "sessions.replay",
        "The raw rrweb recording of one tab of a session, decrypted, in pages, for the player.",
        Access::Permission(Permission::SessionsRead),
    )
    .not_a_tool("Raw DOM snapshots, often megabytes per session; agents read sessions.timeline instead.");

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: ReplayInput) -> Result<Replay> {
        let (project_id, wrapped): (Uuid, String) =
            sqlx::query_as("select p.id, p.data_key from recorded_sessions s join projects p on p.id = s.project_id where s.id = $1")
                .bind(input.id)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(|| not_found("Session"))?;
        let key = project_key(ctx.keys(), project_id, &wrapped)?;
        let chunks: Vec<(i32, Vec<u8>)> = sqlx::query_as(
            "select seq, data from replay_chunks where session_id = $1 and window_id = $2 and seq > $3 order by seq limit 200",
        )
        .bind(input.id)
        .bind(input.window_id)
        .bind(input.after_seq.unwrap_or(-1))
        .fetch_all(&mut *conn)
        .await?;
        let more = chunks.len() == 200;
        let mut events = Vec::new();
        let mut size = 0;
        let mut last = None;
        for (seq, sealed) in &chunks {
            let json = open_chunk(&key, input.id, input.window_id, *seq, sealed)?;
            size += json.len();
            let batch: Vec<Value> = serde_json::from_slice(&json).map_err(AppError::internal)?;
            events.extend(batch);
            last = Some(*seq);
            if size >= REPLAY_PAGE_BYTES {
                break;
            }
        }
        let cut_short = last != chunks.last().map(|c| c.0);
        Ok(Replay { events, next_seq: if more || cut_short { last } else { None } })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BodyInput {
    /// The `id` of a network event.
    pub id: i64,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Bodies {
    pub request_body: Option<String>,
    pub response_body: Option<String>,
}

pub struct RequestBody;

#[async_trait]
impl Service for RequestBody {
    type Input = BodyInput;
    type Output = Bodies;
    const META: Meta = Meta::query(
        "sessions.request_body",
        "Read the recorded request and response body of one network event (at most 10 kB each, masked), in projects that capture bodies.",
        Access::Permission(Permission::SessionsRead),
    )
    .tool();

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: BodyInput) -> Result<Bodies> {
        type Row = (Uuid, DateTime<Utc>, Option<Vec<u8>>, Option<Vec<u8>>, Uuid, String);
        let (session, ts, request, response, project_id, wrapped): Row = sqlx::query_as(
            "select n.session_id, n.ts, n.request_body, n.response_body, p.id, p.data_key
               from network_requests n join projects p on p.id = n.project_id where n.id = $1",
        )
        .bind(input.id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| not_found("Request"))?;
        let key = project_key(ctx.keys(), project_id, &wrapped)?;
        let open = |which, sealed: Option<Vec<u8>>| sealed.map(|s| open_body(&key, session, ts, which, &s)).transpose();
        Ok(Bodies { request_body: open("request", request)?, response_body: open("response", response)? })
    }
}

#[derive(Serialize, JsonSchema)]
pub struct Deleted {
    pub deleted: i64,
}

pub struct Delete;

#[async_trait]
impl Service for Delete {
    type Input = IdInput;
    type Output = Deleted;
    const META: Meta = Meta::mutation(
        "sessions.delete",
        "Delete one recorded session and everything recorded in it. It cannot be undone.",
        Access::Permission(Permission::SessionsDelete),
    )
    .tool()
    .destructive();

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: IdInput) -> Result<Deleted> {
        let session = summary(conn, input.id).await?;
        sqlx::query("delete from recorded_sessions where id = $1").bind(input.id).execute(&mut *conn).await?;
        audit(
            ctx,
            conn,
            Entry::new("sessions.delete", "session", input.id).before(serde_json::json!({
                "projectId": session.project_id, "startedAt": session.started_at, "userRef": session.user_ref, "bytes": session.bytes,
            })),
        )
        .await?;
        Ok(Deleted { deleted: 1 })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteUserInput {
    pub project_id: Uuid,
    /// The id the site passed to `identify()`.
    pub user_ref: String,
}

pub struct DeleteUser;

#[async_trait]
impl Service for DeleteUser {
    type Input = DeleteUserInput;
    type Output = Deleted;
    const META: Meta = Meta::mutation(
        "sessions.delete_user",
        "Delete every session recorded for one user of a project, for example when they ask to be forgotten. It cannot be undone.",
        Access::Permission(Permission::SessionsDelete),
    )
    .tool()
    .destructive();

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: DeleteUserInput) -> Result<Deleted> {
        let user_ref = input.user_ref.trim();
        if user_ref.is_empty() {
            return Err(bad_request("Say which user"));
        }
        let deleted = sqlx::query("delete from recorded_sessions where project_id = $1 and user_ref = $2")
            .bind(input.project_id)
            .bind(user_ref)
            .execute(&mut *conn)
            .await?
            .rows_affected() as i64;
        audit(
            ctx,
            conn,
            Entry::new("sessions.delete_user", "project", input.project_id)
                .metadata(serde_json::json!({ "userRef": user_ref, "sessions": deleted })),
        )
        .await?;
        Ok(Deleted { deleted })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ms: i64) -> EventRow {
        EventRow {
            session_id: Uuid::nil(),
            window_id: Uuid::nil(),
            ts: DateTime::from_timestamp_millis(ms).unwrap_or_default(),
            kind: "page".into(),
            data: Value::Null,
        }
    }

    #[test]
    fn pages_do_not_split_a_timestamp() {
        let mut items: Vec<EventRow> = [1, 2, 3, 3, 3].into_iter().map(row).collect();
        let next = page_end(&mut items, 3);
        assert_eq!(items.len(), 2);
        assert_eq!(next.map(|t| t.timestamp_millis()), Some(2));
        let mut few: Vec<EventRow> = [1, 2].into_iter().map(row).collect();
        assert_eq!(page_end(&mut few, 3), None);
    }
}
