use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::user::User,
    state::AppState,
};

/// The key used to store the authenticated user's ID in the session.
pub const SESSION_USER_ID: &str = "user_id";
/// The key used to store the per-session CSRF token.
pub const CSRF_SESSION_KEY: &str = "csrf_token";
/// Invite token stored for unauthenticated users who visit a /join/:token link.
/// Consumed (and the invite redeemed) immediately after the next successful login or registration.
pub const PENDING_INVITE_KEY: &str = "pending_invite";

/// Store a user's ID into the session (called after successful login).
pub async fn save_user_id(session: &Session, user_id: Uuid) -> AppResult<()> {
    session
        .insert(SESSION_USER_ID, user_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("session insert: {e}")))
}

/// Return the existing CSRF token for this session, or generate and store a new one.
pub async fn get_or_create_csrf_token(session: &Session) -> String {
    if let Ok(Some(token)) = session.get::<String>(CSRF_SESSION_KEY).await {
        return token;
    }
    let token = crate::auth::password::generate_token();
    let _ = session.insert(CSRF_SESSION_KEY, &token).await;
    token
}

/// Constant-time comparison of the submitted CSRF token against the session-stored one.
pub async fn validate_csrf(session: &Session, submitted: &str) -> bool {
    use subtle::ConstantTimeEq;
    if let Ok(Some(stored)) = session.get::<String>(CSRF_SESSION_KEY).await {
        let a = stored.as_bytes();
        let b = submitted.as_bytes();
        a.len() == b.len() && a.ct_eq(b).unwrap_u8() == 1
    } else {
        false
    }
}

/// Remove the user ID from the session (called on logout).
pub async fn clear(session: &Session) -> AppResult<()> {
    session
        .flush()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("session flush: {e}")))
}

// ── CurrentUser extractor ──────────────────────────────────────────────────────
// Use this in handlers that require an authenticated, active user.
//
//   async fn handler(user: CurrentUser) { ... }

pub struct CurrentUser(pub User);

impl std::ops::Deref for CurrentUser {
    type Target = User;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Unauthorized)?;

        let user_id: Option<Uuid> = session
            .get(SESSION_USER_ID)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("session get: {e}")))?;

        let user_id = user_id.ok_or(AppError::Unauthorized)?;

        let app_state = AppState::from_ref(state);
        let user = User::find_by_id(app_state.db(), user_id).await?;

        if !user.is_active() {
            return Err(AppError::Unauthorized);
        }

        Ok(CurrentUser(user))
    }
}

// ── OptionalUser extractor ─────────────────────────────────────────────────────
// Use this in handlers that show different content to logged-in vs anonymous users.
//
//   async fn handler(user: OptionalUser) { if let Some(u) = user.0 { ... } }

pub struct OptionalUser(pub Option<User>);

#[async_trait]
impl<S> FromRequestParts<S> for OptionalUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match CurrentUser::from_request_parts(parts, state).await {
            Ok(CurrentUser(u)) => Ok(OptionalUser(Some(u))),
            Err(AppError::Unauthorized) => Ok(OptionalUser(None)),
            Err(e) => Err(e),
        }
    }
}

// ── RequireSysadmin extractor ──────────────────────────────────────────────────
// Use this in sysadmin-only handlers.
//
//   async fn handler(admin: RequireSysadmin) { ... }

pub struct RequireSysadmin(pub User);

impl std::ops::Deref for RequireSysadmin {
    type Target = User;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for RequireSysadmin
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;
        if !user.is_sysadmin() {
            return Err(AppError::Forbidden);
        }
        Ok(RequireSysadmin(user))
    }
}
