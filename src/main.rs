use axum::{extract::DefaultBodyLimit, http::HeaderValue, Router};
use std::net::SocketAddr;
use time::Duration;
use tokio::net::TcpListener;
use tower_http::set_header::SetResponseHeaderLayer;
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

    // ── S-02: Refuse to start in production with default credentials ──────────
    if config.is_production && config.admin_using_defaults {
        anyhow::bail!(
            "ADMIN_EMAIL and ADMIN_PASSWORD must be set when APP_ENV=production. \
             Refusing to start with default credentials."
        );
    }

    let db = db::create_pool(&config.database_url).await?;

    // Run schema migrations
    sqlx::migrate!("./migrations").run(&db).await?;
    tracing::info!("database migrations applied");

    // Set up session store (creates the tower_sessions table if absent)
    let session_store = PostgresStore::new(db.clone());
    session_store.migrate().await?;
    tracing::info!("session store ready");

    // ── S-08: Reduced session lifetime; S-01: Secure flag for HTTPS ──────────
    let session_layer = SessionManagerLayer::new(session_store)
        .with_expiry(Expiry::OnInactivity(Duration::hours(8)))
        .with_same_site(SameSite::Lax) // Lax required for Google OAuth cross-site redirect
        .with_secure(config.is_production); // Secure flag enforced in production

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
                 ║   Set ADMIN_EMAIL + ADMIN_PASSWORD in .env   ║\n\
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

    // ── S-05: Security headers ────────────────────────────────────────────────
    // SetResponseHeaderLayer::if_not_present lets individual handlers override
    // any header if needed (e.g. asset routes that set their own cache-control).
    let security_headers = tower::ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
        ))
        // HSTS: only send when the app knows it's behind TLS (production mode).
        // Adding HSTS on plain HTTP permanently breaks the site for visitors.
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::STRICT_TRANSPORT_SECURITY,
            if config.is_production {
                HeaderValue::from_static("max-age=63072000; includeSubDomains")
            } else {
                HeaderValue::from_static("max-age=0")
            },
        ));

    // Raise the body limit so multipart file uploads work.
    // Axum's hard default is 2 MB; our app-level check in media.rs is the real enforcer.
    let body_limit = DefaultBodyLimit::max(config.max_upload_bytes as usize);

    let app = Router::new()
        .nest("/api", api)
        .merge(pages_router())
        .layer(security_headers)
        .layer(body_limit)
        .layer(session_layer)
        .with_state(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.app_port).parse()?;
    tracing::info!("listening on {addr}");

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
