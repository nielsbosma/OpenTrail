#![allow(clippy::unwrap_used)]

use opentrail_db::{migrate, migration_state, role_support, sqlx, test_db};

#[tokio::test]
async fn migrates_an_empty_database_and_is_idempotent() {
    let Some(db) = test_db().await else { return };
    let before = migration_state(&db).await.unwrap();
    assert!(before.is_empty_database());
    migrate(&db).await.unwrap();
    migrate(&db).await.unwrap();
    assert!(migration_state(&db).await.unwrap().is_current());
}

#[tokio::test]
async fn audit_log_is_append_only() {
    let Some(db) = test_db().await else { return };
    migrate(&db).await.unwrap();
    sqlx::query("insert into audit_log (actor_label, surface, action, entity_type, prev_hash, hash) values ('system', 'system', 'x.y', 'x', '\\x00', '\\x01')")
        .execute(&db)
        .await
        .unwrap();
    assert!(sqlx::query("update audit_log set action = 'z'").execute(&db).await.is_err());
    assert!(sqlx::query("delete from audit_log").execute(&db).await.is_err());
    assert!(sqlx::query("truncate audit_log").execute(&db).await.is_err());
}

#[tokio::test]
async fn refuses_a_newer_database() {
    let Some(db) = test_db().await else { return };
    migrate(&db).await.unwrap();
    sqlx::query(
        "insert into _sqlx_migrations (version, description, success, checksum, execution_time) values (99999, 'future', true, '\\x00', 0)",
    )
    .execute(&db)
    .await
    .unwrap();
    assert!(matches!(migration_state(&db).await, Err(opentrail_db::DbError::TooNew(99999))));
}

#[tokio::test]
async fn the_role_probe_leaves_nothing_behind() {
    let Some(db) = test_db().await else { return };
    let support = role_support(&db).await.unwrap();
    assert!(support.server_version >= 180000, "Postgres 18 or later, got {}", support.server_version);
    assert!(support.can_create_roles(), "{:?}", support.problem);
    let left: i64 = sqlx::query_scalar("select count(*) from pg_roles where rolname like 'ot_probe_%'").fetch_one(&db).await.unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn a_user_without_createrole_is_told_why() {
    let Some(db) = test_db().await else { return };
    let user = format!("ot_test_plain_{}", std::process::id());
    sqlx::raw_sql(&format!("drop role if exists {user}; create role {user} login password 'plain'")).execute(&db).await.unwrap();
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options = url.parse::<sqlx::postgres::PgConnectOptions>().unwrap().username(&user).password("plain");
    let plain = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
    let support = role_support(&plain).await.unwrap();
    plain.close().await;
    sqlx::raw_sql(&format!("drop role {user}")).execute(&db).await.unwrap();
    assert!(!support.can_create_roles());
    assert!(support.problem.unwrap().contains("permission denied"));
}
