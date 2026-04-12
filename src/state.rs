use std::sync::Arc;

use sqlx::PgPool;

use crate::{
    auth::{email::Mailer, google::GoogleClient},
    config::Config,
};

/// Shared application state cloned into every Axum handler via `State<AppState>`.
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    config: Config,
    db: PgPool,
    mailer: Mailer,
    google_client: Option<GoogleClient>,
    http_client: reqwest::Client,
}

impl AppState {
    pub fn new(
        config: Config,
        db: PgPool,
        mailer: Mailer,
        google_client: Option<GoogleClient>,
    ) -> Self {
        Self(Arc::new(Inner {
            config,
            db,
            mailer,
            google_client,
            http_client: reqwest::Client::new(),
        }))
    }

    pub fn config(&self) -> &Config {
        &self.0.config
    }

    pub fn db(&self) -> &PgPool {
        &self.0.db
    }

    pub fn mailer(&self) -> &Mailer {
        &self.0.mailer
    }

    /// Returns `None` when `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` are not set.
    pub fn google_client(&self) -> Option<&GoogleClient> {
        self.0.google_client.as_ref()
    }

    /// Shared HTTP client for outbound calls (e.g. Google userinfo endpoint).
    pub fn http_client(&self) -> &reqwest::Client {
        &self.0.http_client
    }
}
