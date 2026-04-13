use axum::{extract::DefaultBodyLimit, http::HeaderValue, Router};
use std::{net::SocketAddr, time::Duration as StdDuration};
use time::Duration;
use tokio::net::TcpListener;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tower_sessions::{cookie::SameSite, Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::PostgresStore;
use tracing::Span;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use clanplan::{
    auth::{email::Mailer, google, password as pwd},
    config::Config,
    db,
    routes::{admin_router, auth_router, me_router, pages_router, reunions_router},
    state::AppState,
};

const X_REQUEST_ID: axum::http::HeaderName =
    axum::http::HeaderName::from_static("x-request-id");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "clanplan=info,tower_http=info".into()),
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

    // ── Startup banner ────────────────────────────────────────────────────────
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = if config.is_production { "production" } else { "development" },
        port = config.app_port,
        base_url = %config.app_base_url,
        google_oauth = config.google_oauth_enabled(),
        smtp = %format!("{}:{}", config.smtp_host, config.smtp_port),
        "clanplan starting"
    );

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

    // ── Per-request tracing ───────────────────────────────────────────────────
    // /assets/* requests get a disabled span to suppress static-file log noise.
    // All other requests get an info span carrying method, path, IP (from the
    // X-Real-IP header set by the reverse proxy), user-agent, and request-id.
    // The response callback appends status and latency to the same span record.
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|req: &axum::http::Request<_>| {
            let path = req.uri().path();
            if path.starts_with("/assets/") {
                return Span::none();
            }
            let ip = req
                .headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-");
            let ua = req
                .headers()
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-");
            let req_id = req
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-");
            tracing::info_span!(
                "http",
                method = %req.method(),
                path   = %path,
                req_id = %req_id,
                ip     = %ip,
                ua     = %ua,
            )
        })
        // Suppress the default "started processing request" message; the span
        // fields already capture everything we need at request time.
        .on_request(())
        .on_response(|resp: &axum::http::Response<_>, latency: StdDuration, span: &Span| {
            if span.is_disabled() {
                return;
            }
            let _guard = span.enter();
            tracing::info!(
                status      = resp.status().as_u16(),
                latency_ms  = latency.as_millis(),
                "response"
            );
        });

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

    // Layer order (outermost → innermost for requests):
    //   SetRequestId → PropagateRequestId → Trace → Session → BodyLimit → SecurityHeaders → Router
    let app = Router::new()
        .nest("/api", api)
        .merge(pages_router())
        .layer(security_headers)
        .layer(body_limit)
        .layer(session_layer)
        .layer(trace_layer)
        .layer(PropagateRequestIdLayer::new(X_REQUEST_ID.clone()))
        .layer(SetRequestIdLayer::new(X_REQUEST_ID.clone(), MakeRequestUuid))
        .with_state(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.app_port).parse()?;
    tracing::info!(addr = %addr, "listening");

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
