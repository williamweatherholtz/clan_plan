use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, RedirectUrl, TokenUrl,
};
use serde::Deserialize;

pub type GoogleClient = BasicClient;

/// Construct a Google OAuth2 client from credentials.
/// Panics at startup if the URLs are malformed (they are static constants).
pub fn build_client(
    client_id: &str,
    client_secret: &str,
    redirect_url: &str,
) -> anyhow::Result<GoogleClient> {
    let client = BasicClient::new(
        ClientId::new(client_id.to_owned()),
        Some(ClientSecret::new(client_secret.to_owned())),
        AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".into())
            .map_err(|e| anyhow::anyhow!("bad auth URL: {e}"))?,
        Some(
            TokenUrl::new("https://oauth2.googleapis.com/token".into())
                .map_err(|e| anyhow::anyhow!("bad token URL: {e}"))?,
        ),
    )
    .set_redirect_uri(
        RedirectUrl::new(redirect_url.to_owned())
            .map_err(|e| anyhow::anyhow!("bad redirect URL: {e}"))?,
    );

    Ok(client)
}

/// The subset of Google's `/oauth2/v3/userinfo` response we care about.
#[derive(Debug, Deserialize)]
pub struct GoogleUserInfo {
    /// Google's stable, unique user ID.
    pub sub: String,
    pub email: String,
    pub name: String,
    pub picture: Option<String>,
    pub email_verified: Option<bool>,
}

/// Session keys used during the OAuth2 handshake.
pub const OAUTH_CSRF_KEY: &str = "oauth_csrf";
pub const OAUTH_PKCE_KEY: &str = "oauth_pkce";
