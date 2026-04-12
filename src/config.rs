use anyhow::{Context, Result};
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub session_secret: String,
    pub app_base_url: String,
    pub app_port: u16,

    // Google OAuth2
    pub google_client_id: String,
    pub google_client_secret: String,
    pub google_redirect_url: String,

    // SMTP
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_password: String,
    pub smtp_from: String,
    pub smtp_tls: bool,

    // Storage
    pub media_storage_path: String,
    pub max_upload_bytes: u64,

    // Bootstrap admin account
    pub admin_email: String,
    pub admin_password: String,
    /// True when neither ADMIN_EMAIL nor ADMIN_PASSWORD was overridden.
    pub admin_using_defaults: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: env::var("DATABASE_URL").context("DATABASE_URL is required")?,
            session_secret: env::var("SESSION_SECRET").context("SESSION_SECRET is required")?,
            app_base_url: env::var("APP_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into()),
            app_port: env::var("APP_PORT")
                .unwrap_or_else(|_| "8080".into())
                .parse()
                .context("APP_PORT must be a valid port number")?,

            google_client_id: env::var("GOOGLE_CLIENT_ID").unwrap_or_default(),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default(),
            google_redirect_url: env::var("GOOGLE_REDIRECT_URL")
                .unwrap_or_else(|_| "http://localhost:8080/api/auth/google/callback".into()),

            smtp_host: env::var("SMTP_HOST").unwrap_or_else(|_| "localhost".into()),
            smtp_port: env::var("SMTP_PORT")
                .unwrap_or_else(|_| "1025".into())
                .parse()
                .context("SMTP_PORT must be a number")?,
            smtp_user: env::var("SMTP_USER").unwrap_or_default(),
            smtp_password: env::var("SMTP_PASSWORD").unwrap_or_default(),
            smtp_from: env::var("SMTP_FROM")
                .unwrap_or_else(|_| "familyer@localhost".into()),
            smtp_tls: env::var("SMTP_TLS")
                .unwrap_or_else(|_| "false".into())
                .eq_ignore_ascii_case("true"),

            media_storage_path: env::var("MEDIA_STORAGE_PATH")
                .unwrap_or_else(|_| "./media".into()),
            max_upload_bytes: env::var("MAX_UPLOAD_BYTES")
                .unwrap_or_else(|_| "104857600".into())
                .parse()
                .context("MAX_UPLOAD_BYTES must be a number")?,

            admin_email: env::var("ADMIN_EMAIL")
                .unwrap_or_else(|_| "admin@localhost".into()),
            admin_password: env::var("ADMIN_PASSWORD")
                .unwrap_or_else(|_| "password".into()),
            admin_using_defaults: env::var("ADMIN_EMAIL").is_err()
                && env::var("ADMIN_PASSWORD").is_err(),
        })
    }

    /// Returns true when Google OAuth credentials are configured.
    pub fn google_oauth_enabled(&self) -> bool {
        !self.google_client_id.is_empty() && !self.google_client_secret.is_empty()
    }
}
