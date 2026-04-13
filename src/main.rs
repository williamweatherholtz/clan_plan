use axum::{extract::DefaultBodyLimit, Router};
use std::net::SocketAddr;
use time::Duration;
use tokio::net::TcpListener;
use tower_sessions::{cookie::SameSite, Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::PostgresStore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use clanplan::{
    auth::{email::Mailer, google, password as pwd},
    config::Config,
    db,
    routes::{admin_router, auth_router, me_router, pages_router, reunions_router},
    state::AppState,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "clanplan=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env()?;
    let db = db::create_pool(&config.database_url).await?;

    // Run schema migrations
    sqlx::migrate!("./migrations").run(&db).await?;
    tracing::info!("database migrations applied");

    // Set up session store (creates the tower_sessions table if absent)
    let session_store = PostgresStore::new(db.clone());
    session_store.migrate().await?;
    tracing::info!("session store ready");

    let session_layer = SessionManagerLayer::new(session_store)
        .with_expiry(Expiry::OnInactivity(Duration::days(7)))
        .with_same_site(SameSite::Lax);

    // Email transport
    let mailer = Mailer::new(&config)?;

    // Google OAuth2 client (optional — only if credentials are present)
    let google_client = if config.google_oauth_enabled() {
        tracing::info!("Google OAuth2 enabled");
        Some(google::build_client(
            &config.google_client_id,
            &config.google_client_secret,
            &config.google_redirect_url,
        )?)
    } else {
        tracing::info!("Google OAuth2 disabled (no credentials configured)");
        None
    };

    // ── Bootstrap: create a sysadmin if none exists ───────────────────────────
    let (sysadmin_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM users WHERE role = 'sysadmin'")
            .fetch_one(&db)
            .await?;

    if sysadmin_count == 0 {
        let hash = pwd::hash_password(&config.admin_password).await?;
        sqlx::query(
            r#"INSERT INTO users (email, display_name, password_hash, role, email_verified_at)
               VALUES ($1, 'Admin', $2, 'sysadmin', NOW())"#,
        )
        .bind(&config.admin_email)
        .bind(&hash)
        .execute(&db)
        .await?;

        if config.admin_using_defaults {
            tracing::warn!(
                "\n\
                 ╔══════════════════════════════════════════════╗\n\
                 ║       DEFAULT ADMIN CREDENTIALS IN USE       ║\n\
                 ║   Change the password after first login!     ║\n\
                 ╠══════════════════════════════════════════════╣\n\
                 ║  Email:    admin@localhost                    ║\n\
                 ║  Password: password                          ║\n\
                 ╚══════════════════════════════════════════════╝"
            );
        } else {
            tracing::info!(email = %config.admin_email, "admin account created");
        }
    }

    let state = AppState::new(config.clone(), db, mailer, google_client);

    let api = Router::new()
        .nest("/auth", auth_router())
        .merge(me_router())
        .merge(admin_router())
        .nest("/reunions", reunions_router());

    // Raise the body limit so multipart file uploads work.
    // Axum's hard default is 2 MB; our app-level check in media.rs is the real enforcer.
    let body_limit = DefaultBodyLimit::max(config.max_upload_bytes as usize);

    let app = Router::new()
        .nest("/api", api)
        .merge(pages_router())
        .layer(body_limit)
        .layer(session_layer)
        .with_state(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.app_port).parse()?;
    tracing::info!("listening on {addr}");

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
