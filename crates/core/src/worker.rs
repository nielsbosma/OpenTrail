use std::sync::Arc;

use chrono::Duration;
use serde_json::json;

use crate::context::{App, Ctx, Surface};
use crate::error::{AppError, Result};
use crate::jobs::{self, Job, enqueue};

const LEASE_SECONDS: i64 = 300;
const SWEEP_MINUTES: i64 = 10;

async fn run(ctx: &Ctx, job: &Job) -> Result<()> {
    let mut tx = ctx.db().begin().await?;
    match job.kind.as_str() {
        "sweep" => sweep(ctx, &mut tx).await?,
        other => return Err(AppError::internal(anyhow::anyhow!("unknown job kind {other}"))),
    }
    tx.commit().await?;
    Ok(())
}

/// Clears expired rows. `run_next` schedules the next sweep once this one is done, since a pending
/// job holds its unique key.
async fn sweep(ctx: &Ctx, conn: &mut sqlx::PgConnection) -> Result<()> {
    sqlx::query("delete from idempotency_keys where created_at < $1 - interval '1 day'").bind(ctx.now).execute(&mut *conn).await?;
    sqlx::query("delete from used_tokens where expires_at < $1").bind(ctx.now).execute(&mut *conn).await?;
    sqlx::query("delete from login_failures where last_failed_at < $1 - interval '2 days' and (locked_until is null or locked_until < $1)")
        .bind(ctx.now)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "delete from oauth_clients c where c.created_at < $1 - interval '7 days'
           and not exists (select 1 from oauth_grants g where g.client_id = c.client_id)
           and not exists (select 1 from oauth_codes o where o.client_id = c.client_id)",
    )
    .bind(ctx.now)
    .execute(&mut *conn)
    .await?;
    expire_sessions(ctx, conn).await?;
    Ok(())
}

/// Sessions past their project's retention, a bounded number per sweep so one sweep stays short.
pub async fn expire_sessions(ctx: &Ctx, conn: &mut sqlx::PgConnection) -> Result<u64> {
    Ok(sqlx::query(
        "delete from recorded_sessions where id in (
           select s.id from recorded_sessions s join projects p on p.id = s.project_id
            where s.started_at < $1 - make_interval(days => p.retention_days)
            limit 5000)",
    )
    .bind(ctx.now)
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

pub async fn schedule_sweep(conn: &mut sqlx::PgConnection, at: Option<chrono::DateTime<chrono::Utc>>) -> Result<()> {
    enqueue(conn, "sweep", json!({}), at, Some("sweep")).await
}

/// Claims and runs one due job. Returns whether there was one.
pub async fn run_next(app: &Arc<App>) -> Result<bool> {
    let job = {
        let mut conn = app.db.acquire().await?;
        jobs::claim(&mut conn, LEASE_SECONDS).await?
    };
    let Some(job) = job else { return Ok(false) };
    let ctx = Ctx::new(app.clone(), Surface::Worker);
    let result = run(&ctx, &job).await;
    let mut conn = app.db.acquire().await?;
    match result {
        Ok(()) => jobs::complete(&mut conn, job.id).await?,
        Err(e) => {
            tracing::warn!(job = %job.id, kind = %job.kind, attempt = job.attempts, error = %e, cause = ?e.cause, "job failed");
            if job.kind == "sweep" {
                jobs::complete(&mut conn, job.id).await?;
            } else {
                jobs::fail(&mut conn, &job, &e.to_string()).await?;
            }
        }
    }
    if job.kind == "sweep" {
        schedule_sweep(&mut conn, Some(crate::context::now() + Duration::minutes(SWEEP_MINUTES))).await?;
    }
    Ok(true)
}

/// Runs every due job, for tests and one-shot tools.
pub async fn drain(app: &Arc<App>) -> Result<usize> {
    let mut count = 0;
    while run_next(app).await? {
        count += 1;
        if count > 1000 {
            break;
        }
    }
    Ok(count)
}
