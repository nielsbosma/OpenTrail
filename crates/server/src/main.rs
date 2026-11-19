use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use opentrail_core::context::App;
use opentrail_core::env::{Env, ProcessRole};
use opentrail_core::net::SafeHttp;
use opentrail_core::settings::SettingsCache;
use opentrail_crypto::Keys;
use opentrail_server::{AppState, router};
use tracing_subscriber::EnvFilter;

async fn worker_loop(app: Arc<App>) {
    if let Ok(mut conn) = app.db.acquire().await {
        let _ = opentrail_core::worker::schedule_sweep(&mut conn, None).await;
    }
    loop {
        match opentrail_core::worker::run_next(&app).await {
            Ok(true) => {}
            Ok(false) => tokio::time::sleep(Duration::from_secs(2)).await,
            Err(e) => {
                tracing::warn!(error = %e, "worker could not claim a job");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}

/// `opentrail health`: the image's health check, since the runtime image has no shell or curl.
fn health_check() -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3000);
    let mut stream = std::net::TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(
        b"GET /api/health HTTP/1.0
Host: localhost

",
    )?;
    let mut head = [0u8; 12];
    stream.read_exact(&mut head)?;
    anyhow::ensure!(head.ends_with(b" 200"), "unhealthy: {}", String::from_utf8_lossy(&head));
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("schema") {
        use std::io::Write;
        writeln!(std::io::stdout(), "{}", serde_json::to_string_pretty(&opentrail_server::service_schemas())?)?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("health") {
        return health_check();
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")))
        .init();
    let env = match Env::from_process() {
        Ok(env) => env,
        Err(problems) => {
            for p in &problems {
                tracing::error!("{}: {} {}", p.name, p.message, p.hint);
            }
            anyhow::bail!("OpenTrail cannot start until the environment is fixed");
        }
    };
    let keys = Keys::derive(&env.app_secret).context("APP_SECRET")?;
    let db = opentrail_db::connect(&env.database_url, 20).await?;
    opentrail_db::migrate(&db).await.context("database migrations")?;
    match opentrail_db::role_support(&db).await {
        Ok(s) if s.can_create_roles() => tracing::info!(postgres = s.server_version, "the database user can create roles"),
        Ok(s) => tracing::warn!(
            postgres = s.server_version,
            problem = s.problem,
            "the database user cannot create roles, so SQL access stays off"
        ),
        Err(e) => tracing::warn!("could not check whether the database user can create roles: {e}"),
    }
    let settings = SettingsCache::from_env(std::env::vars());
    let users: i64 = sqlx::query_scalar("select count(*) from users").fetch_one(&db).await?;
    if users == 0 {
        let url = env.app_url.as_deref().unwrap_or("the app");
        if env.setup_code.is_some() {
            tracing::warn!("No admin yet. Open {url} with the setup link from your deployment, or enter SETUP_CODE from the environment");
        } else {
            tracing::warn!("No admin yet. Open {url} and enter the setup code {}", keys.setup_code());
        }
    }
    let port = env.port;
    let role = env.role;
    let app = Arc::new(App { env, db, keys, settings, http: SafeHttp::new() });

    if role != ProcessRole::Web {
        tokio::spawn(worker_loop(app.clone()));
    }
    if role == ProcessRole::Worker {
        shutdown().await;
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await?;
    tracing::info!(version = opentrail_core::env::app_version(), "listening on port {port}");
    axum::serve(listener, router(AppState::new(app)).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}
