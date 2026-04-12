use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    Json,
};
use oauth2::{
    AuthorizationCode, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::{
    auth::{
        google::{GoogleUserInfo, OAUTH_CSRF_KEY, OAUTH_PKCE_KEY},
        password,
        session::{self, CurrentUser},
    },
    error::{AppError, AppResult},
    models::user::{EmailVerification, NewUser, PasswordReset, User},
    state::AppState,
};

// ── PATCH /me ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UpdateMeRequest {
    pub display_name: Option<String>,
    /// Pass an empty string to clear the avatar.
    pub avatar_url: Option<String>,
}

pub async fn update_me(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(body): Json<UpdateMeRequest>,
) -> AppResult<impl IntoResponse> {
    if let Some(display_name) = &body.display_name {
        let trimmed = display_name.trim();
        if trimmed.is_empty() {
            return Err(AppError::BadRequest("display_name cannot be empty".into()));
        }
        User::update_display_name(state.db(), user.id, trimmed).await?;
    }

    if let Some(avatar_url) = &body.avatar_url {
        let url = if avatar_url.trim().is_empty() {
            None
        } else {
            Some(avatar_url.as_str())
        };
        User::set_avatar(state.db(), user.id, url).await?;
    }

    let updated = User::find_by_id(state.db(), user.id).await?;
    Ok(Json(updated))
}

// ── Request / response shapes ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub display_name: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct VerifyEmailParams {
    pub token: String,
}

#[derive(Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

#[derive(Deserialize)]
pub struct GoogleCallbackParams {
    pub code: String,
    pub state: String,
}

#[derive(Serialize)]
struct MessageResponse {
    message: &'static str,
}

// ── POST /auth/register ────────────────────────────────────────────────────────

pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> AppResult<impl IntoResponse> {
    password::validate_password(&body.password)
        .map_err(|e| AppError::BadRequest(e.into()))?;

    if body.display_name.trim().is_empty() {
        return Err(AppError::BadRequest("display name cannot be empty".into()));
    }

    let hash = password::hash_password(&body.password)
        .await
        .map_err(|e| AppError::Internal(e))?;

    let user = User::create(
        state.db(),
        NewUser {
            email: body.email.clone(),
            display_name: body.display_name.trim().to_owned(),
            password_hash: Some(hash),
            google_id: None,
            family_unit_id: None,
            avatar_url: None,
        },
    )
    .await?;

    // Send verification email; don't fail the request if delivery fails
    let token = password::generate_token();
    if let Err(e) = EmailVerification::create(state.db(), user.id, &token).await {
        tracing::error!("failed to store verification token: {e:?}");
    } else {
        let verify_url = format!(
            "{}/api/auth/verify-email?token={token}",
            state.config().app_base_url
        );
        if let Err(e) = state
            .mailer()
            .send_verification_email(&user.email, &user.display_name, &verify_url)
            .await
        {
            tracing::error!("failed to send verification email to {}: {e:?}", user.email);
        }
    }

    Ok((StatusCode::CREATED, Json(user)))
}

// ── POST /auth/login ───────────────────────────────────────────────────────────

pub async fn login(
    State(state): State<AppState>,
    session: Session,
    Json(body): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    // Use a generic error message to prevent email enumeration
    let auth_err = || AppError::Unauthorized;

    let user = User::find_by_email(state.db(), &body.email)
        .await?
        .ok_or_else(auth_err)?;

    if !user.is_active() {
        return Err(auth_err());
    }

    let hash = user.password_hash.as_deref().ok_or_else(|| {
        // Account was created via Google — no password set
        AppError::BadRequest("this account uses Google login; use the Google sign-in button".into())
    })?;

    if !password::verify_password(&body.password, hash).await {
        return Err(auth_err());
    }

    session::save_user_id(&session, user.id).await?;

    Ok(Json(user))
}

// ── POST /auth/logout ──────────────────────────────────────────────────────────

pub async fn logout(
    _user: CurrentUser,
    session: Session,
) -> AppResult<impl IntoResponse> {
    session::clear(&session).await?;
    Ok(Redirect::to("/login"))
}

// ── GET /auth/verify-email?token=… ────────────────────────────────────────────

pub async fn verify_email(
    State(state): State<AppState>,
    Query(params): Query<VerifyEmailParams>,
) -> AppResult<impl IntoResponse> {
    let verification = EmailVerification::consume(state.db(), &params.token).await?;
    User::mark_email_verified(state.db(), verification.user_id).await?;
    // In a template-rendered app this redirects; for now return JSON
    Ok(Json(MessageResponse {
        message: "email verified — you can now log in",
    }))
}

// ── POST /auth/forgot-password ─────────────────────────────────────────────────

pub async fn forgot_password(
    State(state): State<AppState>,
    Json(body): Json<ForgotPasswordRequest>,
) -> AppResult<impl IntoResponse> {
    // Always return 200 — never reveal whether the email exists
    if let Ok(Some(user)) = User::find_by_email(state.db(), &body.email).await {
        if user.is_active() {
            let token = password::generate_token();
            if let Err(e) = PasswordReset::create(state.db(), user.id, &token).await {
                tracing::error!("failed to create password reset token: {e:?}");
            } else {
                let reset_url = format!(
                    "{}/reset-password?token={token}",
                    state.config().app_base_url
                );
                if let Err(e) = state
                    .mailer()
                    .send_password_reset_email(&user.email, &user.display_name, &reset_url)
                    .await
                {
                    tracing::error!("failed to send reset email to {}: {e:?}", user.email);
                }
            }
        }
    }

    Ok(Json(MessageResponse {
        message: "if an account with that email exists, a reset link has been sent",
    }))
}

// ── POST /auth/reset-password ──────────────────────────────────────────────────

pub async fn reset_password(
    State(state): State<AppState>,
    Json(body): Json<ResetPasswordRequest>,
) -> AppResult<impl IntoResponse> {
    password::validate_password(&body.new_password)
        .map_err(|e| AppError::BadRequest(e.into()))?;

    let reset = PasswordReset::consume(state.db(), &body.token).await?;

    let hash = password::hash_password(&body.new_password)
        .await
        .map_err(|e| AppError::Internal(e))?;

    User::update_password_hash(state.db(), reset.user_id, &hash).await?;

    Ok(Json(MessageResponse {
        message: "password updated — you can now log in",
    }))
}

// ── GET /auth/google ───────────────────────────────────────────────────────────

pub async fn google_start(
    State(state): State<AppState>,
    session: Session,
) -> AppResult<impl IntoResponse> {
    let client = state
        .google_client()
        .ok_or_else(|| AppError::BadRequest("Google login is not configured".into()))?;

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    session
        .insert(OAUTH_CSRF_KEY, csrf_token.secret().clone())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("session: {e}")))?;
    session
        .insert(OAUTH_PKCE_KEY, pkce_verifier.secret().clone())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("session: {e}")))?;

    Ok(Redirect::to(auth_url.as_str()))
}

// ── GET /auth/google/callback?code=…&state=… ──────────────────────────────────

pub async fn google_callback(
    State(state): State<AppState>,
    session: Session,
    Query(params): Query<GoogleCallbackParams>,
) -> AppResult<impl IntoResponse> {
    let client = state
        .google_client()
        .ok_or_else(|| AppError::BadRequest("Google login is not configured".into()))?;

    // Retrieve and consume the stored OAuth handshake data
    let stored_csrf: Option<String> = session
        .remove(OAUTH_CSRF_KEY)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("session: {e}")))?;
    let stored_pkce: Option<String> = session
        .remove(OAUTH_PKCE_KEY)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("session: {e}")))?;

    // Verify CSRF state
    let stored_csrf =
        stored_csrf.ok_or_else(|| AppError::BadRequest("OAuth state missing or expired".into()))?;
    if stored_csrf != params.state {
        return Err(AppError::BadRequest("OAuth state mismatch".into()));
    }

    let pkce_verifier = PkceCodeVerifier::new(
        stored_pkce.ok_or_else(|| AppError::BadRequest("PKCE verifier missing".into()))?,
    );

    // Exchange authorization code for access token
    let token_response = client
        .exchange_code(AuthorizationCode::new(params.code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("token exchange: {e}")))?;

    // Fetch user profile from Google
    let user_info: GoogleUserInfo = state
        .http_client()
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(token_response.access_token().secret())
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("userinfo fetch: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("userinfo parse: {e}")))?;

    let user = find_or_create_google_user(state.db(), &user_info).await?;

    session::save_user_id(&session, user.id).await?;

    Ok(Redirect::to("/"))
}

// ── Helper: find or create user from Google profile ───────────────────────────

async fn find_or_create_google_user(
    pool: &sqlx::PgPool,
    info: &GoogleUserInfo,
) -> AppResult<User> {
    // 1. Existing Google-linked account
    if let Some(user) = User::find_by_google_id(pool, &info.sub).await? {
        return Ok(user);
    }

    // 2. Existing email account — link Google ID to it
    if let Some(user) = User::find_by_email(pool, &info.email).await? {
        User::attach_google_id(pool, user.id, &info.sub).await?;
        // Also verify email if not already done (Google guarantees email_verified)
        if !user.is_email_verified() {
            User::mark_email_verified(pool, user.id).await?;
        }
        return User::find_by_id(pool, user.id).await;
    }

    // 3. Brand-new account
    let user = User::create(
        pool,
        NewUser {
            email: info.email.clone(),
            display_name: info.name.clone(),
            password_hash: None,
            google_id: Some(info.sub.clone()),
            family_unit_id: None,
            avatar_url: info.picture.clone(),
        },
    )
    .await?;

    // Google-created accounts are pre-verified
    User::mark_email_verified(pool, user.id).await?;
    User::find_by_id(pool, user.id).await
}

// ── GET /me ────────────────────────────────────────────────────────────────────

pub async fn get_me(user: CurrentUser) -> impl IntoResponse {
    Json(user.0)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_request_deserializes() {
        let json = r#"{"email":"a@b.com","display_name":"Alice","password":"secret123"}"#;
        let req: RegisterRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "a@b.com");
    }

    #[test]
    fn login_request_deserializes() {
        let json = r#"{"email":"a@b.com","password":"secret123"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.password, "secret123");
    }

    #[test]
    fn update_me_partial_deserializes() {
        let json = r#"{"display_name":"Alice"}"#;
        let req: UpdateMeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.display_name.as_deref(), Some("Alice"));
        assert!(req.avatar_url.is_none());
    }

    #[test]
    fn update_me_avatar_clear() {
        let json = r#"{"avatar_url":""}"#;
        let req: UpdateMeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.avatar_url.as_deref(), Some(""));
    }
}
