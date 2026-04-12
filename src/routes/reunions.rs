use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::session::{CurrentUser, RequireSysadmin},
    error::{AppError, AppResult},
    models::{
        feedback::SurveyQuestion,
        reunion::{NewReunion, Reunion, ReunionDate},
        user::User,
    },
    phase::Phase,
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion};

// ── Response types ─────────────────────────────────────────────────────────────

/// Reunion plus its confirmed date range (if set).
#[derive(Serialize)]
pub struct ReunionDetail {
    #[serde(flatten)]
    pub reunion: Reunion,
    pub dates: Option<ReunionDate>,
}

// ── Request types ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UpdateReunionRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    /// Sysadmin-only: reassign the responsible admin.
    pub responsible_admin_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct SetDatesRequest {
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
}

// ── GET /reunions ──────────────────────────────────────────────────────────────

pub async fn list_reunions(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> AppResult<impl IntoResponse> {
    let reunions = Reunion::list_all(state.db()).await?;
    Ok(Json(reunions))
}

// ── POST /reunions ─────────────────────────────────────────────────────────────

pub async fn create_reunion(
    RequireSysadmin(admin): RequireSysadmin,
    State(state): State<AppState>,
    Json(body): Json<NewReunion>,
) -> AppResult<impl IntoResponse> {
    if body.title.trim().is_empty() {
        return Err(AppError::BadRequest("title cannot be empty".into()));
    }
    let reunion = Reunion::create(state.db(), body, admin.id).await?;
    Ok((StatusCode::CREATED, Json(reunion)))
}

// ── GET /reunions/:id ──────────────────────────────────────────────────────────

pub async fn get_reunion(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    let dates = ReunionDate::find_for_reunion(state.db(), reunion_id).await?;
    Ok(Json(ReunionDetail { reunion, dates }))
}

// ── PATCH /reunions/:id ────────────────────────────────────────────────────────

pub async fn update_reunion(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<UpdateReunionRequest>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &reunion)?;

    let title = body.title.as_deref().unwrap_or(&reunion.title);
    // `None` in body means "keep current"; we can't distinguish "set to null"
    // for description here — a separate PATCH field would be needed for that edge case.
    let description = body
        .description
        .as_deref()
        .or(reunion.description.as_deref());

    let updated = Reunion::update_title_description(state.db(), reunion_id, title, description).await?;

    // Sysadmin-only: change the RA assignment
    if let Some(ra_id) = body.responsible_admin_id {
        if !user.is_sysadmin() {
            return Err(AppError::Forbidden);
        }
        Reunion::set_responsible_admin(state.db(), reunion_id, Some(ra_id)).await?;
        return Ok(Json(Reunion::find_by_id(state.db(), reunion_id).await?));
    }

    Ok(Json(updated))
}

// ── POST /reunions/:id/advance-phase ──────────────────────────────────────────

pub async fn advance_phase(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &reunion)?;

    // Phase-specific precondition checks
    match &reunion.phase {
        Phase::Availability => {
            let dates = ReunionDate::find_for_reunion(state.db(), reunion_id).await?;
            if dates.is_none() {
                return Err(AppError::BadRequest(
                    "set the reunion date range (POST /reunions/:id/dates) before advancing".into(),
                ));
            }
        }
        Phase::Locations => {
            if reunion.selected_location_id.is_none() {
                return Err(AppError::BadRequest(
                    "select a winning location (POST /reunions/:id/locations/:loc_id/select) before advancing".into(),
                ));
            }
        }
        _ => {}
    }

    let updated = Reunion::advance_phase(state.db(), reunion_id, &reunion.phase).await?;

    // Post-transition side effects
    if updated.phase == Phase::PostReunion {
        if let Err(e) = SurveyQuestion::seed_defaults(state.db(), reunion_id).await {
            tracing::error!("failed to seed survey questions for {reunion_id}: {e:?}");
        }
    }

    // Broadcast phase notification emails in the background (non-fatal)
    {
        let state = state.clone();
        let reunion_title = updated.title.clone();
        let phase_label = updated.phase.label().to_string();
        tokio::spawn(async move {
            let users = match User::list_active_verified(state.db()).await {
                Ok(u) => u,
                Err(e) => {
                    tracing::error!("phase notification: failed to load users: {e:?}");
                    return;
                }
            };
            let app_url = &state.config().app_base_url;
            for user in &users {
                if let Err(e) = state
                    .mailer()
                    .send_phase_notification(
                        &user.email,
                        &user.display_name,
                        &reunion_title,
                        &phase_label,
                        app_url,
                    )
                    .await
                {
                    tracing::warn!(
                        "phase notification to {}: {e:?}",
                        user.email
                    );
                }
            }
        });
    }

    Ok(Json(updated))
}

// ── POST /reunions/:id/dates ───────────────────────────────────────────────────
// RA sets (or replaces) the confirmed date range.
// This does NOT automatically advance the phase — the RA explicitly calls
// advance-phase when satisfied with the dates.

pub async fn set_dates(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<SetDatesRequest>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &reunion)?;

    // Dates can be set while in availability phase (or earlier for prep)
    if !matches!(
        reunion.phase,
        Phase::Draft | Phase::Availability | Phase::DateSelected
    ) {
        return Err(AppError::BadRequest(
            "dates can only be set during draft, availability, or date_selected phases".into(),
        ));
    }

    if body.end_date < body.start_date {
        return Err(AppError::BadRequest(
            "end_date must be on or after start_date".into(),
        ));
    }

    let dates = ReunionDate::set(
        state.db(),
        reunion_id,
        body.start_date,
        body.end_date,
        user.id,
    )
    .await?;

    Ok(Json(dates))
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn date_validation() {
        let start = NaiveDate::from_ymd_opt(2026, 7, 10).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap();
        assert!(end >= start);
        assert!(!(start > end));
    }

    #[test]
    fn update_request_deserializes() {
        let json = r#"{"title":"Summer Reunion 2026"}"#;
        let req: UpdateReunionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.title.unwrap(), "Summer Reunion 2026");
        assert!(req.description.is_none());
    }
}
