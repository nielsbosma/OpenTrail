use async_trait::async_trait;
use chrono::{DateTime, Utc};
use opentrail_crypto::{DataKey, random_bytes};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use super::Ok_;
use crate::audit::{Entry, audit};
use crate::context::Ctx;
use crate::error::{Result, bad_request, not_found};
use crate::permissions::Permission;
use crate::recording::INGEST_KEY_PREFIX;
use crate::service::{Access, Meta, Service};

const MAX_ORIGINS: usize = 50;

#[derive(Serialize, JsonSchema, sqlx::FromRow, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub id: Uuid,
    pub name: String,
    /// Exact origins a browser may send recordings from.
    pub allowed_origins: Vec<String>,
    pub retention_days: i32,
    pub capture_bodies: bool,
    pub capture_canvas: bool,
    pub monthly_session_quota: i32,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    /// Sessions recorded in the last 30 days.
    pub recent_sessions: i64,
}

const PROJECT: &str = "select p.id, p.name, p.allowed_origins, p.retention_days, p.capture_bodies, p.capture_canvas, p.monthly_session_quota,
        p.archived_at is not null as archived, p.created_at,
        (select count(*) from recorded_sessions s where s.project_id = p.id and s.started_at > now() - interval '30 days') as recent_sessions
   from projects p";

async fn project(conn: &mut PgConnection, id: Uuid) -> Result<ProjectView> {
    sqlx::query_as(&format!("{PROJECT} where p.id = $1")).bind(id).fetch_optional(&mut *conn).await?.ok_or_else(|| not_found("Project"))
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListInput {
    /// Include archived projects.
    #[serde(default)]
    pub include_archived: bool,
}

pub struct List;

#[async_trait]
impl Service for List {
    type Input = ListInput;
    type Output = Vec<ProjectView>;
    const META: Meta = Meta::query(
        "projects.list",
        "List the projects sessions are recorded in, with their settings and how many sessions they recorded in the last 30 days. Use a project's id to filter sessions and events.",
        Access::Permission(Permission::SessionsRead),
    )
    .tool();

    async fn run(_ctx: &Ctx, conn: &mut PgConnection, input: ListInput) -> Result<Vec<ProjectView>> {
        Ok(sqlx::query_as(&format!("{PROJECT} where $1 or p.archived_at is null order by lower(p.name)"))
            .bind(input.include_archived)
            .fetch_all(&mut *conn)
            .await?)
    }
}

#[derive(Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct KeyView {
    pub id: Uuid,
    pub name: String,
    /// Public: it goes in the page's source and can only send recordings to this project.
    pub key: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    #[serde(flatten)]
    pub project: ProjectView,
    pub keys: Vec<KeyView>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdInput {
    pub id: Uuid,
}

pub struct Get;

#[async_trait]
impl Service for Get {
    type Input = IdInput;
    type Output = ProjectDetail;
    const META: Meta = Meta::query(
        "projects.get",
        "Get one project with its settings and its public ingest keys, which a site puts in its recording snippet.",
        Access::Permission(Permission::ProjectsManage),
    )
    .tool();

    async fn run(_ctx: &Ctx, conn: &mut PgConnection, input: IdInput) -> Result<ProjectDetail> {
        let project = project(conn, input.id).await?;
        let keys = sqlx::query_as(
            "select id, name, key, last_used_at, created_at from ingest_keys where project_id = $1 and revoked_at is null order by created_at",
        )
        .bind(input.id)
        .fetch_all(&mut *conn)
        .await?;
        Ok(ProjectDetail { project, keys })
    }
}

/// `scheme://host[:port]` exactly as a browser sends it in `Origin`.
pub fn normalize_origin(raw: &str) -> Result<String> {
    let invalid = || bad_request("{origin} is not an origin like https://app.example.com").params([("origin", &raw.trim())]);
    let url = url::Url::parse(raw.trim()).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    Ok(url.origin().ascii_serialization())
}

fn normalize_origins(origins: &[String]) -> Result<Vec<String>> {
    if origins.len() > MAX_ORIGINS {
        return Err(bad_request(format!("A project can have at most {MAX_ORIGINS} origins")));
    }
    let mut out: Vec<String> = origins.iter().filter(|o| !o.trim().is_empty()).map(|o| normalize_origin(o)).collect::<Result<_>>()?;
    out.sort();
    out.dedup();
    Ok(out)
}

fn check_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(bad_request("A project name is 1 to 100 characters"));
    }
    Ok(name.to_string())
}

fn check_retention(days: i32) -> Result<i32> {
    if !(1..=3650).contains(&days) {
        return Err(bad_request("Retention is 1 to 3650 days"));
    }
    Ok(days)
}

fn new_key() -> String {
    format!("{INGEST_KEY_PREFIX}{}", hex::encode(random_bytes::<16>()))
}

async fn insert_key(ctx: &Ctx, conn: &mut PgConnection, project_id: Uuid, name: &str) -> Result<KeyView> {
    let id = Uuid::now_v7();
    let key = new_key();
    let view: KeyView = sqlx::query_as(
        "insert into ingest_keys (id, project_id, key, name) values ($1, $2, $3, $4) returning id, name, key, last_used_at, created_at",
    )
    .bind(id)
    .bind(project_id)
    .bind(&key)
    .bind(name)
    .fetch_one(&mut *conn)
    .await?;
    audit(
        ctx,
        conn,
        Entry::new("projects.create_key", "ingest_key", id).after(serde_json::json!({ "projectId": project_id, "name": name })),
    )
    .await?;
    Ok(view)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateInput {
    pub name: String,
    /// Origins like `https://app.example.com` that may send recordings.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    pub retention_days: Option<i32>,
}

pub struct Create;

#[async_trait]
impl Service for Create {
    type Input = CreateInput;
    type Output = ProjectDetail;
    const META: Meta = Meta::mutation(
        "projects.create",
        "Create a project to record a site in, with the origins allowed to send recordings. Returns the project with its first public ingest key.",
        Access::Permission(Permission::ProjectsManage),
    )
    .tool();

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: CreateInput) -> Result<ProjectDetail> {
        let name = check_name(&input.name)?;
        let origins = normalize_origins(&input.allowed_origins)?;
        let retention = check_retention(input.retention_days.unwrap_or(90))?;
        let id = Uuid::now_v7();
        let wrapped = ctx.keys().wrap_data_key(&DataKey::generate(), &id.to_string());
        sqlx::query("insert into projects (id, name, allowed_origins, retention_days, data_key) values ($1, $2, $3, $4, $5)")
            .bind(id)
            .bind(&name)
            .bind(&origins)
            .bind(retention)
            .bind(wrapped)
            .execute(&mut *conn)
            .await?;
        audit(
            ctx,
            conn,
            Entry::new("projects.create", "project", id)
                .after(serde_json::json!({ "name": name, "allowedOrigins": origins, "retentionDays": retention })),
        )
        .await?;
        let key = insert_key(ctx, conn, id, "Default").await?;
        Ok(ProjectDetail { project: project(conn, id).await?, keys: vec![key] })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInput {
    pub id: Uuid,
    pub name: Option<String>,
    pub allowed_origins: Option<Vec<String>>,
    pub retention_days: Option<i32>,
    /// Store request and response bodies (at most 10 kB each, masked and encrypted).
    pub capture_bodies: Option<bool>,
    pub capture_canvas: Option<bool>,
    pub monthly_session_quota: Option<i32>,
}

pub struct Update;

#[async_trait]
impl Service for Update {
    type Input = UpdateInput;
    type Output = ProjectView;
    const META: Meta = Meta::mutation(
        "projects.update",
        "Change a project's name, allowed origins, retention, body or canvas capture, or monthly session quota. Shorter retention deletes older sessions at the next sweep.",
        Access::Permission(Permission::ProjectsManage),
    )
    .tool()
    .destructive();

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: UpdateInput) -> Result<ProjectView> {
        let before = project(conn, input.id).await?;
        let name = input.name.as_deref().map(check_name).transpose()?.unwrap_or(before.name.clone());
        let origins = input.allowed_origins.as_deref().map(normalize_origins).transpose()?.unwrap_or(before.allowed_origins.clone());
        let retention = input.retention_days.map(check_retention).transpose()?.unwrap_or(before.retention_days);
        let quota = input.monthly_session_quota.unwrap_or(before.monthly_session_quota);
        if quota < 1 {
            return Err(bad_request("The monthly quota must be at least 1 session"));
        }
        sqlx::query(
            "update projects set name = $2, allowed_origins = $3, retention_days = $4, capture_bodies = $5, capture_canvas = $6,
                    monthly_session_quota = $7, updated_at = now() where id = $1",
        )
        .bind(input.id)
        .bind(&name)
        .bind(&origins)
        .bind(retention)
        .bind(input.capture_bodies.unwrap_or(before.capture_bodies))
        .bind(input.capture_canvas.unwrap_or(before.capture_canvas))
        .bind(quota)
        .execute(&mut *conn)
        .await?;
        let after = project(conn, input.id).await?;
        audit(ctx, conn, Entry::new("projects.update", "project", input.id).before(settings_of(&before)).after(settings_of(&after)))
            .await?;
        Ok(after)
    }
}

fn settings_of(p: &ProjectView) -> serde_json::Value {
    serde_json::json!({
        "name": p.name, "allowedOrigins": p.allowed_origins, "retentionDays": p.retention_days, "captureBodies": p.capture_bodies,
        "captureCanvas": p.capture_canvas, "monthlySessionQuota": p.monthly_session_quota,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ArchiveInput {
    pub id: Uuid,
    /// False restores an archived project.
    #[serde(default = "yes")]
    pub archived: bool,
}

fn yes() -> bool {
    true
}

pub struct Archive;

#[async_trait]
impl Service for Archive {
    type Input = ArchiveInput;
    type Output = ProjectView;
    const META: Meta = Meta::mutation(
        "projects.archive",
        "Archive a project so it stops accepting recordings, or restore it with archived false. Recorded sessions stay until their retention ends.",
        Access::Permission(Permission::ProjectsManage),
    )
    .tool()
    .destructive();

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: ArchiveInput) -> Result<ProjectView> {
        let done = sqlx::query(
            "update projects set archived_at = case when $2 then coalesce(archived_at, now()) end, updated_at = now() where id = $1",
        )
        .bind(input.id)
        .bind(input.archived)
        .execute(&mut *conn)
        .await?;
        if done.rows_affected() == 0 {
            return Err(not_found("Project"));
        }
        let action = if input.archived { "projects.archive" } else { "projects.restore" };
        audit(ctx, conn, Entry::new(action, "project", input.id)).await?;
        project(conn, input.id).await
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateKeyInput {
    pub project_id: Uuid,
    pub name: String,
}

pub struct CreateKey;

#[async_trait]
impl Service for CreateKey {
    type Input = CreateKeyInput;
    type Output = KeyView;
    const META: Meta = Meta::mutation(
        "projects.create_key",
        "Create another public ingest key for a project, for example one per site or environment so one can be revoked alone.",
        Access::Permission(Permission::ProjectsManage),
    )
    .tool();

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: CreateKeyInput) -> Result<KeyView> {
        project(conn, input.project_id).await?;
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(bad_request("A key name is 1 to 100 characters"));
        }
        insert_key(ctx, conn, input.project_id, name).await
    }
}

pub struct RevokeKey;

#[async_trait]
impl Service for RevokeKey {
    type Input = IdInput;
    type Output = Ok_;
    const META: Meta = Meta::mutation(
        "projects.revoke_key",
        "Revoke a public ingest key. Sites that still use it stop being recorded at once.",
        Access::Permission(Permission::ProjectsManage),
    )
    .tool()
    .destructive();

    async fn run(ctx: &Ctx, conn: &mut PgConnection, input: IdInput) -> Result<Ok_> {
        let project: Option<Uuid> =
            sqlx::query_scalar("update ingest_keys set revoked_at = now() where id = $1 and revoked_at is null returning project_id")
                .bind(input.id)
                .fetch_optional(&mut *conn)
                .await?;
        let Some(project) = project else { return Err(not_found("Key")) };
        audit(ctx, conn, Entry::new("projects.revoke_key", "ingest_key", input.id).metadata(serde_json::json!({ "projectId": project })))
            .await?;
        Ok(Ok_::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(normalize_origin("https://App.Example.com/").ok().as_deref(), Some("https://app.example.com"));
        assert_eq!(normalize_origin("http://localhost:5173").ok().as_deref(), Some("http://localhost:5173"));
        assert_eq!(normalize_origin("https://a.test:443").ok().as_deref(), Some("https://a.test"));
        for bad in ["app.example.com", "https://a.test/path", "ftp://a.test", "https://u:p@a.test", "https://a.test?x=1", "*"] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
        assert!(new_key().starts_with(INGEST_KEY_PREFIX) && new_key().len() == INGEST_KEY_PREFIX.len() + 32);
    }
}
