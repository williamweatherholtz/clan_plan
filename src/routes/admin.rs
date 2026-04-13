use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::session::RequireSysadmin,
    error::{AppError, AppResult},
    models::{
        app_settings::AppSettings,
        host_rotation::{HostRotation, NewHostRotation},
        reunion::Reunion,
        user::{FamilyUnit, User, UserRole},
    },
    phase::Phase,
    state::AppState,
};

// ── GET /admin/users ──────────────────────────────────────────────────────────

pub async fn list_users(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
) -> AppResult<impl IntoResponse> {
    let users = User::list_all(state.db()).await?;
    Ok(Json(users))
}

// ── PATCH /admin/users/:id ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    /// Promote/demote the user's role.
    pub role: Option<UserRole>,
    /// Deactivate (true) or reactivate (false) the account.
    pub deactivated: Option<bool>,
    /// Assign or clear family unit.
    /// - key absent / not provided → don't touch it
    /// - `"family_unit_id": null`  → clear (set to NULL in DB)
    /// - `"family_unit_id": "uuid"` → assign to that unit
    #[serde(default, deserialize_with = "deserialize_optional_uuid")]
    pub family_unit_id: Option<Option<Uuid>>,
}

/// Distinguishes a missing JSON key (outer None) from an explicit `null`
/// (Some(None)) vs an actual UUID value (Some(Some(uuid))).
fn deserialize_optional_uuid<'de, D>(d: D) -> Result<Option<Option<Uuid>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<Uuid>::deserialize(d)?))
}

pub async fn update_user(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> AppResult<impl IntoResponse> {
    // Verify user exists before applying any change
    User::find_by_id(state.db(), user_id).await?;

    if let Some(role) = &body.role {
        User::set_role(state.db(), user_id, role).await?;
    }
    if let Some(deactivated) = body.deactivated {
        User::set_deactivated(state.db(), user_id, deactivated).await?;
    }
    // Some(inner) means the key was present; inner is the desired value (None = clear)
    if let Some(inner) = body.family_unit_id {
        User::set_family_unit(state.db(), user_id, inner).await?;
    }

    let updated = User::find_by_id(state.db(), user_id).await?;
    Ok(Json(updated))
}

// ── GET /admin/family-units ───────────────────────────────────────────────────

pub async fn list_family_units(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
) -> AppResult<impl IntoResponse> {
    let units = FamilyUnit::list_all(state.db()).await?;
    Ok(Json(units))
}

// ── POST /admin/family-units ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateFamilyUnitRequest {
    pub name: String,
}

pub async fn create_family_unit(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
    Json(body): Json<CreateFamilyUnitRequest>,
) -> AppResult<impl IntoResponse> {
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("name cannot be empty".into()));
    }
    let unit = FamilyUnit::create(state.db(), body.name.trim()).await?;
    Ok((StatusCode::CREATED, Json(unit)))
}

// ── PATCH /admin/family-units/:id ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RenameFamilyUnitRequest {
    pub name: String,
}

pub async fn rename_family_unit(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
    Path(unit_id): Path<Uuid>,
    Json(body): Json<RenameFamilyUnitRequest>,
) -> AppResult<impl IntoResponse> {
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("name cannot be empty".into()));
    }
    let unit = FamilyUnit::rename(state.db(), unit_id, body.name.trim()).await?;
    Ok(Json(unit))
}

// ── GET /admin/host-rotation ──────────────────────────────────────────────────

pub async fn list_host_rotation(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
) -> AppResult<impl IntoResponse> {
    let entries = HostRotation::list_all(state.db()).await?;
    Ok(Json(entries))
}

// ── POST /admin/host-rotation ─────────────────────────────────────────────────

pub async fn add_host_rotation(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
    Json(body): Json<NewHostRotation>,
) -> AppResult<impl IntoResponse> {
    let entry = HostRotation::create(state.db(), body).await?;
    Ok((StatusCode::CREATED, Json(entry)))
}

// ── POST /admin/host-rotation/:id/set-next ────────────────────────────────────

pub async fn set_next_host(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
    Path(entry_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let entry = HostRotation::set_next(state.db(), entry_id).await?;
    Ok(Json(entry))
}

// ── DELETE /admin/host-rotation/:id ──────────────────────────────────────────

pub async fn delete_host_rotation_entry(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
    Path(entry_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    HostRotation::delete(state.db(), entry_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── POST /admin/reunions/:id/set-phase ───────────────────────────────────────
// Emergency override: set a reunion to any phase, bypassing all preconditions.

#[derive(Deserialize)]
pub struct ForcePhaseRequest {
    pub phase: Phase,
}

pub async fn force_set_phase(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<ForcePhaseRequest>,
) -> AppResult<impl IntoResponse> {
    let updated = Reunion::force_set_phase(state.db(), reunion_id, &body.phase).await?;
    Ok(Json(updated))
}

// ── PATCH /admin/registration ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UpdateRegistrationRequest {
    pub enabled: bool,
}

pub async fn update_registration(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
    Json(body): Json<UpdateRegistrationRequest>,
) -> AppResult<impl IntoResponse> {
    AppSettings::set_registration_enabled(state.db(), body.enabled).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── GET /admin/config ─────────────────────────────────────────────────────────
// Returns non-secret runtime configuration for observability.
// Secrets (DATABASE_URL, SESSION_SECRET, SMTP credentials, OAuth keys) are omitted.

#[derive(Serialize)]
pub struct ConfigView {
    pub app_base_url: String,
    pub app_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_from: String,
    pub smtp_tls: bool,
    pub google_oauth_enabled: bool,
    pub google_redirect_url: String,
    pub media_storage_path: String,
    pub max_upload_bytes: u64,
}

pub async fn get_config(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let c = state.config();
    Json(ConfigView {
        app_base_url: c.app_base_url.clone(),
        app_port: c.app_port,
        smtp_host: c.smtp_host.clone(),
        smtp_port: c.smtp_port,
        smtp_from: c.smtp_from.clone(),
        smtp_tls: c.smtp_tls,
        google_oauth_enabled: c.google_oauth_enabled(),
        google_redirect_url: c.google_redirect_url.clone(),
        media_storage_path: c.media_storage_path.clone(),
        max_upload_bytes: c.max_upload_bytes,
    })
}

// ── GET /admin/storage ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StorageStats {
    pub total_bytes: i64,
    pub total_files: i64,
}

pub async fn storage_stats(
    _admin: RequireSysadmin,
    State(state): State<AppState>,
) -> AppResult<impl IntoResponse> {
    let (total_bytes, total_files): (Option<i64>, i64) = sqlx::query_as(
        "SELECT SUM(file_size_bytes), COUNT(*) FROM media",
    )
    .fetch_one(state.db())
    .await
    .map_err(crate::error::AppError::Database)?;

    Ok(Json(StorageStats {
        total_bytes: total_bytes.unwrap_or(0),
        total_files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_user_request_partial_role() {
        let json = r#"{"role":"sysadmin"}"#;
        let req: UpdateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.role, Some(UserRole::Sysadmin));
        assert!(req.deactivated.is_none());
        assert!(req.family_unit_id.is_none());
    }

    #[test]
    fn update_user_request_deactivate() {
        let json = r#"{"deactivated":true}"#;
        let req: UpdateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.deactivated, Some(true));
        assert!(req.role.is_none());
    }

    #[test]
    fn create_family_unit_request_deserializes() {
        let json = r#"{"name":"The Smith Family"}"#;
        let req: CreateFamilyUnitRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "The Smith Family");
    }

    #[test]
    fn rename_family_unit_request_deserializes() {
        let json = r#"{"name":"The Jones Family"}"#;
        let req: RenameFamilyUnitRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "The Jones Family");
    }

    #[test]
    fn force_phase_request_deserializes() {
        let json = r#"{"phase":"active"}"#;
        let req: ForcePhaseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.phase, Phase::Active);
    }

    #[test]
    fn force_phase_archived_deserializes() {
        let json = r#"{"phase":"archived"}"#;
        let req: ForcePhaseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.phase, Phase::Archived);
    }
}
