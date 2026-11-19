#![allow(clippy::unwrap_used)]

use opentrail_core::jobs::enqueue;
use opentrail_core::testing::app;
use opentrail_core::worker;
use serde_json::json;

#[tokio::test]
async fn the_sweep_schedules_itself_again() {
    let Some(app) = app().await else { return };
    let mut conn = app.db.acquire().await.unwrap();
    worker::schedule_sweep(&mut conn, None).await.unwrap();
    worker::drain(&app).await.unwrap();
    let next: Vec<bool> =
        sqlx::query_scalar("select run_at > now() from jobs where kind = 'sweep' and done_at is null").fetch_all(&mut *conn).await.unwrap();
    assert_eq!(next, vec![true], "one future sweep is pending");
}

#[tokio::test]
async fn an_exhausted_job_does_not_block_its_key() {
    let Some(app) = app().await else { return };
    let mut conn = app.db.acquire().await.unwrap();
    enqueue(&mut conn, "example", json!({ "n": 1 }), None, Some("k")).await.unwrap();
    enqueue(&mut conn, "example", json!({ "n": 2 }), None, Some("k")).await.unwrap();
    let payload: serde_json::Value =
        sqlx::query_scalar("select payload from jobs where unique_key = 'k'").fetch_one(&mut *conn).await.unwrap();
    assert_eq!(payload, json!({ "n": 1 }), "a pending job keeps its key");

    sqlx::query("update jobs set attempts = max_attempts, last_error = 'boom' where unique_key = 'k'").execute(&mut *conn).await.unwrap();
    enqueue(&mut conn, "example", json!({ "n": 3 }), None, Some("k")).await.unwrap();
    let (payload, attempts): (serde_json::Value, i32) =
        sqlx::query_as("select payload, attempts from jobs where unique_key = 'k'").fetch_one(&mut *conn).await.unwrap();
    assert_eq!((payload, attempts), (json!({ "n": 3 }), 0));
}

#[tokio::test]
async fn the_sweep_clears_expired_rows() {
    let Some(app) = app().await else { return };
    let mut conn = app.db.acquire().await.unwrap();
    let (old, new) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
    sqlx::query("insert into used_tokens (id, expires_at) values ($1, now() - interval '1 minute'), ($2, now() + interval '1 hour')")
        .bind(old)
        .bind(new)
        .execute(&mut *conn)
        .await
        .unwrap();
    worker::schedule_sweep(&mut conn, None).await.unwrap();
    worker::drain(&app).await.unwrap();
    let left: Vec<uuid::Uuid> = sqlx::query_scalar("select id from used_tokens").fetch_all(&mut *conn).await.unwrap();
    assert_eq!(left, [new]);
}
