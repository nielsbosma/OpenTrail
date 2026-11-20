use std::sync::Arc;

use opentrail_crypto::Keys;

use crate::context::{App, Ctx, Surface};
use crate::env::{Env, ProcessRole};
use crate::net::SafeHttp;
use crate::settings::SettingsCache;

pub const TEST_SECRET: &str = "test-secret-test-secret-test-secret-0123";

/// An app on a fresh database schema, or `None` when `TEST_DATABASE_URL`
/// is not set (the test is then skipped).
pub async fn app() -> Option<Arc<App>> {
    app_with(true).await
}

/// As `app`, with the outbound rules a real install has.
pub async fn production_app() -> Option<Arc<App>> {
    app_with(false).await
}

async fn app_with(development: bool) -> Option<Arc<App>> {
    let db = opentrail_db::test_db().await?;
    opentrail_db::migrate(&db).await.expect("migrations apply");
    let env = Env {
        database_url: String::new(),
        app_secret: TEST_SECRET.into(),
        app_url: Some("http://localhost:3000".into()),
        setup_code: None,
        port: 3000,
        role: ProcessRole::All,
        development,
    };
    Some(Arc::new(App {
        env,
        db,
        keys: Keys::derive(TEST_SECRET).expect("test secret"),
        settings: SettingsCache::default(),
        http: SafeHttp::new(),
    }))
}

pub fn system_ctx(app: &Arc<App>) -> Ctx {
    Ctx::new(app.clone(), Surface::System)
}

/// Someone signed in to the web app with this role. No user row exists behind it.
pub fn signed_in(app: &Arc<App>, role: crate::permissions::Role) -> Ctx {
    Ctx::new(app.clone(), Surface::Web).with_actor(crate::context::Actor {
        user_id: uuid::Uuid::now_v7(),
        email: format!("{}@example.com", role.as_str()),
        name: role.as_str().into(),
        role,
        permissions: role.permissions(),
        must_enable_two_factor: false,
        credential: crate::context::Credential::Session,
    })
}
