#![allow(clippy::unwrap_used)]

use std::io::Write;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use opentrail_core::services::{projects, sessions};
use opentrail_core::testing::{app, signed_in};
use opentrail_core::{Role, call};
use opentrail_server::{AppState, router};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const SITE: &str = "https://shop.example";

fn gzip(value: &Value) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(value.to_string().as_bytes()).unwrap();
    e.finish().unwrap()
}

async fn post(router: &axum::Router, key: &str, origin: &str, body: Vec<u8>) -> (StatusCode, Option<String>) {
    let request = Request::post(format!("/i/v1/batch?k={key}"))
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::ORIGIN, origin)
        .header(header::USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/142.0.0.0 Safari/537.36")
        .header("x-forwarded-for", "203.0.113.50")
        .body(Body::from(body))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let allow = response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).map(|v| v.to_str().unwrap().to_string());
    let status = response.status();
    let _ = response.into_body().collect().await;
    (status, allow)
}

fn batch(session: Uuid, window: Uuid, seq: i32, start: i64) -> Value {
    json!({
        "v": 1, "sessionId": session, "windowId": window, "seq": seq, "release": "1.2.3", "lang": "sv-SE",
        "events": [
            { "type": 4, "timestamp": start, "data": { "href": "https://shop.example/", "width": 1280, "height": 720 } },
            { "type": 2, "timestamp": start + 1, "data": { "node": { "type": 0, "id": 1, "childNodes": [
                { "type": 2, "id": 2, "tagName": "p", "attributes": { "onclick": "steal()" }, "childNodes": [
                    { "type": 3, "id": 3, "textContent": "Personnummer 19811218-9876" } ] } ] }, "initialOffset": { "top": 0, "left": 0 } } },
            { "type": 5, "timestamp": start + 2, "data": { "tag": "trail/page", "payload": { "url": "https://shop.example/checkout?token=abc", "title": "Checkout" } } },
            { "type": 5, "timestamp": start + 3, "data": { "tag": "trail/el", "payload": { "id": 2, "role": "button", "name": "Pay Now", "testid": "pay" } } },
            { "type": 3, "timestamp": start + 4, "data": { "source": 2, "type": 2, "id": 2, "x": 100, "y": 200 } },
            { "type": 6, "timestamp": start + 5, "data": { "plugin": "rrweb/console@1", "payload": { "level": "error", "trace": [], "payload": ["\"card 4111 1111 1111 1111\""] } } },
            { "type": 5, "timestamp": start + 6, "data": { "tag": "trail/net", "payload": { "initiator": "fetch", "method": "POST", "url": "/api/pay", "status": 502, "durationMs": 30, "requestBody": "{\"pnr\":\"811218-9876\"}", "responseBody": "bad gateway" } } },
            { "type": 5, "timestamp": start + 7, "data": { "tag": "trail/error", "payload": { "kind": "unhandledrejection", "message": "payment failed", "stack": "Error: payment failed\n  at pay (app.js:1:1)" } } },
            { "type": 5, "timestamp": start + 8, "data": { "tag": "trail/identify", "payload": { "userRef": "customer-7" } } }
        ]
    })
}

fn create(name: &str, retention_days: Option<i32>) -> projects::CreateInput {
    projects::CreateInput { name: name.into(), allowed_origins: vec![format!("{SITE}/")], retention_days }
}

#[tokio::test]
async fn a_batch_is_stored_masked_and_readable() {
    let Some(app) = app().await else { return };
    let admin = signed_in(&app, Role::Admin);
    let project = call::<projects::Create>(&admin, create("Shop", None)).await.unwrap();
    assert_eq!(project.project.allowed_origins, vec![SITE.to_string()]);
    let key = project.keys[0].key.clone();
    let update = projects::UpdateInput {
        id: project.project.id,
        name: None,
        allowed_origins: None,
        retention_days: None,
        capture_bodies: Some(true),
        capture_canvas: None,
        monthly_session_quota: None,
    };
    call::<projects::Update>(&admin, update).await.unwrap();

    let router = router(AppState::new(app.clone()));
    let (session, window) = (Uuid::now_v7(), Uuid::now_v7());
    let start = chrono::Utc::now().timestamp_millis() - 10_000;
    let body = gzip(&batch(session, window, 0, start));

    assert_eq!(post(&router, &key, "https://evil.example", body.clone()).await, (StatusCode::FORBIDDEN, None), "other origins are refused");
    assert_eq!(post(&router, "trk_pub_nope", SITE, body.clone()).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(post(&router, &key, SITE, body.clone()).await, (StatusCode::NO_CONTENT, Some(SITE.to_string())));
    assert_eq!(post(&router, &key, SITE, body).await.0, StatusCode::NO_CONTENT, "a retry is accepted and changes nothing");
    let stale = gzip(&batch(Uuid::now_v7(), window, 0, start - 3 * 86_400_000));
    assert_eq!(post(&router, &key, SITE, stale).await.0, StatusCode::BAD_REQUEST, "timestamps far from now");

    let viewer = signed_in(&app, Role::Viewer);
    let detail = call::<sessions::Get>(&viewer, sessions::IdInput { id: session }).await.unwrap();
    let s = &detail.session;
    assert_eq!((s.page_count, s.error_count, s.failed_request_count, detail.windows.len()), (1, 1, 1, 1));
    assert_eq!(s.user_ref.as_deref(), Some("customer-7"));
    assert_eq!(s.browser.as_deref(), Some("Chrome 142"));
    assert_eq!(s.ip_prefix.as_deref(), Some("203.0.113.0/24"));
    assert_eq!((s.screen_width, s.screen_height), (Some(1280), Some(720)));
    assert_eq!(s.first_url.as_deref(), Some("https://shop.example/checkout?token=****"));

    let timeline = call::<sessions::GetTimeline>(&viewer, sessions::TimelineInput { id: session, kinds: None, after: None, limit: None })
        .await
        .unwrap();
    let kinds: Vec<&str> = timeline.items.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["page", "interaction", "console", "network", "error"]);
    assert_eq!(timeline.items[1].data["name"], "Pay Now", "the click knows its element");
    assert_eq!(timeline.items[2].data["message"], "card **** **** **** ****");
    let request_id = timeline.items[3].data["id"].as_i64().unwrap();
    assert_eq!(timeline.items[3].data["hasRequestBody"], true);

    let bodies = call::<sessions::RequestBody>(&viewer, sessions::BodyInput { id: request_id }).await.unwrap();
    assert_eq!(bodies.request_body.as_deref(), Some("{\"pnr\":\"******-****\"}"));
    assert_eq!(bodies.response_body.as_deref(), Some("bad gateway"));

    let replay =
        call::<sessions::GetReplay>(&viewer, sessions::ReplayInput { id: session, window_id: window, after_seq: None }).await.unwrap();
    assert_eq!(replay.events.len(), 9);
    assert!(replay.next_seq.is_none());
    let text = serde_json::to_string(&replay.events).unwrap();
    assert!(text.contains("********-****") && !text.contains("19811218"), "masked in the replay");
    assert!(!text.contains("steal()"), "handlers are removed");
    assert!(!text.contains("requestBody"), "bodies are not in the replay");
    let stored: Vec<u8> =
        sqlx::query_scalar("select data from replay_chunks where session_id = $1").bind(session).fetch_one(&app.db).await.unwrap();
    assert!(!String::from_utf8_lossy(&stored).contains("shop.example"), "chunks are encrypted at rest");

    let search = sessions::SearchInput {
        kind: sessions::EventKind::Network,
        project_id: Some(project.project.id),
        from: None,
        to: None,
        text: Some("/api/".into()),
        level: None,
        min_status: None,
        failed: Some(true),
        limit: None,
        before: None,
    };
    let failed = call::<sessions::Search>(&viewer, search).await.unwrap();
    assert_eq!(failed.items.len(), 1);
    assert_eq!(failed.items[0].session_id, session);

    let group = sessions::GroupInput {
        kind: sessions::EventKind::Error,
        by: sessions::GroupBy::Message,
        project_id: Some(project.project.id),
        from: None,
        to: None,
        text: None,
        limit: None,
    };
    let groups = call::<sessions::GroupEvents>(&viewer, group).await.unwrap();
    assert_eq!((groups[0].value.as_deref(), groups[0].count, groups[0].sessions), (Some("payment failed"), 1, 1));

    let list = sessions::ListInput { url: Some("CHECKOUT".into()), has_errors: Some(true), ..Default::default() };
    assert_eq!(call::<sessions::List>(&viewer, list).await.unwrap().items.len(), 1);

    assert!(call::<sessions::Delete>(&viewer, sessions::IdInput { id: session }).await.is_err(), "a viewer cannot delete");
    let forget = sessions::DeleteUserInput { project_id: project.project.id, user_ref: "customer-7".into() };
    assert_eq!(call::<sessions::DeleteUser>(&admin, forget).await.unwrap().deleted, 1);
    let left: i64 =
        sqlx::query_scalar("select count(*) from pageviews where session_id = $1").bind(session).fetch_one(&app.db).await.unwrap();
    assert_eq!(left, 0, "deleting a session deletes its events");
}

#[tokio::test]
async fn sessions_end_at_the_quota_retention_and_archive() {
    let Some(app) = app().await else { return };
    let admin = signed_in(&app, Role::Admin);
    let project = call::<projects::Create>(&admin, create("Small", Some(1))).await.unwrap();
    let id = project.project.id;
    let key = project.keys[0].key.clone();
    sqlx::query("update projects set monthly_session_quota = 1 where id = $1").bind(id).execute(&app.db).await.unwrap();
    let router = router(AppState::new(app.clone()));
    let now = chrono::Utc::now().timestamp_millis();
    let first = Uuid::now_v7();
    assert_eq!(post(&router, &key, SITE, gzip(&batch(first, Uuid::now_v7(), 0, now - 1000))).await.0, StatusCode::NO_CONTENT);
    assert_eq!(post(&router, &key, SITE, gzip(&batch(Uuid::now_v7(), Uuid::now_v7(), 0, now))).await.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        post(&router, &key, SITE, gzip(&batch(first, Uuid::now_v7(), 0, now))).await.0,
        StatusCode::NO_CONTENT,
        "an existing session goes on"
    );

    sqlx::query("update recorded_sessions set started_at = now() - interval '2 days' where id = $1")
        .bind(first)
        .execute(&app.db)
        .await
        .unwrap();
    let ctx = opentrail_core::testing::system_ctx(&app);
    let mut conn = app.db.acquire().await.unwrap();
    assert_eq!(opentrail_core::worker::expire_sessions(&ctx, &mut conn).await.unwrap(), 1);

    call::<projects::Archive>(&admin, projects::ArchiveInput { id, archived: true }).await.unwrap();
    assert_eq!(post(&router, &key, SITE, gzip(&batch(Uuid::now_v7(), Uuid::now_v7(), 0, now))).await.0, StatusCode::FORBIDDEN);
    call::<projects::Archive>(&admin, projects::ArchiveInput { id, archived: false }).await.unwrap();

    let other = call::<projects::Create>(&admin, create("Other", None)).await.unwrap();
    let shared = Uuid::now_v7();
    assert_eq!(post(&router, &other.keys[0].key, SITE, gzip(&batch(shared, Uuid::now_v7(), 0, now))).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        post(&router, &key, SITE, gzip(&batch(shared, Uuid::now_v7(), 0, now))).await.0,
        StatusCode::FORBIDDEN,
        "a session cannot be continued from another project"
    );
}

#[tokio::test]
async fn preflight_answers_only_allowed_origins() {
    let Some(app) = app().await else { return };
    let admin = signed_in(&app, Role::Admin);
    let project = call::<projects::Create>(&admin, create("P", None)).await.unwrap();
    let router = router(AppState::new(app.clone()));
    for (origin, status) in [(SITE, StatusCode::NO_CONTENT), ("https://evil.example", StatusCode::FORBIDDEN)] {
        let request =
            Request::options(format!("/i/v1/batch?k={}", project.keys[0].key)).header(header::ORIGIN, origin).body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status, "{origin}");
    }
}
