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
        reunion::{NewReunion, Reunion, ReunionAdmin, ReunionDate, ReunionFamilyUnit},
        user::{FamilyUnit, User},
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
    /// URL slug for /r/:slug friendly links. Pass null to clear.
    pub slug: Option<String>,
    /// RA: set the availability poll window. Both must be present to update.
    pub avail_poll_start: Option<NaiveDate>,
    pub avail_poll_end: Option<NaiveDate>,
    /// Pass true to clear the poll window back to defaults.
    pub clear_avail_poll: Option<bool>,
    /// RA: override the assumed duration for activities with no explicit end time.
    pub default_activity_duration_minutes: Option<i32>,
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
    // Seed default survey questions at creation so the RA can review/remove them before PostReunion.
    if let Err(e) = SurveyQuestion::seed_defaults(state.db(), reunion.id).await {
        tracing::warn!("failed to seed survey questions for {}: {e:?}", reunion.id);
    }
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
    ensure_ra(&user, &state, reunion_id).await?;

    let title = body.title.as_deref().unwrap_or(&reunion.title);
    let description = body
        .description
        .as_deref()
        .or(reunion.description.as_deref());

    let updated = Reunion::update_title_description(state.db(), reunion_id, title, description).await?;

    // Update slug if provided (empty string → clear)
    if let Some(slug) = &body.slug {
        let trimmed = slug.trim();
        let slug_val = if trimmed.is_empty() { None } else { Some(trimmed) };
        Reunion::set_slug(state.db(), reunion_id, slug_val).await?;
    }

    // RA: update or clear the availability poll window
    if body.clear_avail_poll == Some(true) {
        Reunion::set_avail_poll_window(state.db(), reunion_id, None, None).await?;
    } else if let (Some(s), Some(e)) = (body.avail_poll_start, body.avail_poll_end) {
        if e < s {
            return Err(AppError::BadRequest("poll end must be on or after start".into()));
        }
        Reunion::set_avail_poll_window(state.db(), reunion_id, Some(s), Some(e)).await?;
    }

    // RA: override the assumed activity duration
    if let Some(mins) = body.default_activity_duration_minutes {
        if mins < 1 {
            return Err(AppError::BadRequest("duration must be at least 1 minute".into()));
        }
        Reunion::set_default_activity_duration(state.db(), reunion_id, mins).await?;
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
    ensure_ra(&user, &state, reunion_id).await?;

    // Dates must be set before moving from Availability to Locations/Schedule.
    if reunion.phase == Phase::Availability {
        let dates = ReunionDate::find_for_reunion(state.db(), reunion_id).await?;
        if dates.is_none() {
            return Err(AppError::BadRequest(
                "set the reunion date range before advancing".into(),
            ));
        }
    }

    let updated = Reunion::advance_phase(state.db(), reunion_id, &reunion.phase).await?;

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

// ── POST /reunions/:id/retreat-phase ─────────────────────────────────────────

pub async fn retreat_phase(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let prev = reunion.phase.retreat()?;
    let updated = Reunion::set_phase(state.db(), reunion_id, &prev).await?;
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
    ensure_ra(&user, &state, reunion_id).await?;

    // Dates can be set at any non-archived phase — RA may need to enter dates
    // that were agreed out-of-band (e.g. by SMS), regardless of current phase.
    if matches!(reunion.phase, Phase::Archived) {
        return Err(AppError::BadRequest("cannot modify an archived reunion".into()));
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

// ── POST /reunions/:id/unarchive ─────────────────────────────────────────────

pub async fn unarchive(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;
    if reunion.phase != Phase::Archived {
        return Err(AppError::BadRequest("reunion is not archived".into()));
    }
    let updated = Reunion::force_set_phase(state.db(), reunion_id, &Phase::PostReunion).await?;
    Ok(Json(updated))
}

// ── DELETE /reunions/:id ──────────────────────────────────────────────────────

pub async fn delete_reunion(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    // Only sysadmin may delete; RA can only archive
    if !user.is_sysadmin() {
        return Err(AppError::Forbidden);
    }
    let _ = reunion; // silence unused warning
    Reunion::delete(state.db(), reunion_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── GET /reunions/:id/my-completion ──────────────────────────────────────────

#[derive(Serialize)]
pub struct CompletionResponse {
    pub availability: bool,
    pub locations: bool,
    pub expenses: bool,
    pub survey: bool,
}

pub async fn my_completion(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let availability: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM availability WHERE reunion_id = $1 AND user_id = $2",
    )
    .bind(reunion_id)
    .bind(user.id)
    .fetch_one(state.db())
    .await?;

    let locations: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM location_votes lv
         JOIN location_candidates lc ON lc.id = lv.location_candidate_id
         WHERE lc.reunion_id = $1 AND lv.user_id = $2",
    )
    .bind(reunion_id)
    .bind(user.id)
    .fetch_one(state.db())
    .await?;

    let expenses: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM expense_confirmations WHERE reunion_id = $1 AND user_id = $2",
    )
    .bind(reunion_id)
    .bind(user.id)
    .fetch_one(state.db())
    .await?;

    let survey: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM survey_responses sr
         JOIN survey_questions sq ON sq.id = sr.survey_question_id
         WHERE sq.reunion_id = $1 AND sr.user_id = $2",
    )
    .bind(reunion_id)
    .bind(user.id)
    .fetch_one(state.db())
    .await?;

    Ok(Json(CompletionResponse { availability, locations, expenses, survey }))
}

// ── GET /reunions/:id/family-units ────────────────────────────────────────────

pub async fn list_reunion_family_units(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    let ids = ReunionFamilyUnit::list_ids_for_reunion(state.db(), reunion_id).await?;
    let all_units = FamilyUnit::list_all(state.db()).await?;
    let units: Vec<FamilyUnit> = all_units.into_iter().filter(|u| ids.contains(&u.id)).collect();
    Ok(Json(units))
}

// ── PUT /reunions/:id/family-units/:fu_id ─────────────────────────────────────

pub async fn add_reunion_family_unit(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, fu_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    super::helpers::ensure_ra(&user, &state, reunion_id).await?;
    // Verify the family unit exists
    FamilyUnit::find_by_id(state.db(), fu_id).await?;
    ReunionFamilyUnit::add(state.db(), reunion_id, fu_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── DELETE /reunions/:id/family-units/:fu_id ──────────────────────────────────

pub async fn remove_reunion_family_unit(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, fu_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    super::helpers::ensure_ra(&user, &state, reunion_id).await?;
    ReunionFamilyUnit::remove(state.db(), reunion_id, fu_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── GET /reunions/:id/setup-progress ─────────────────────────────────────────

#[derive(Serialize)]
pub struct ProgressCount {
    pub done: i64,
    pub total: i64,
}

#[derive(Serialize)]
pub struct SetupProgressResponse {
    pub availability: ProgressCount,
    pub locations: ProgressCount,
    pub survey: ProgressCount,
}

pub async fn setup_progress(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reunion_family_units WHERE reunion_id = $1",
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await?;

    if total == 0 {
        return Ok(Json(SetupProgressResponse {
            availability: ProgressCount { done: 0, total: 0 },
            locations:    ProgressCount { done: 0, total: 0 },
            survey:       ProgressCount { done: 0, total: 0 },
        }));
    }

    let avail_done: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(DISTINCT rfu.family_unit_id)
           FROM reunion_family_units rfu
           WHERE rfu.reunion_id = $1
             AND EXISTS (
               SELECT 1 FROM availability a
               JOIN users u ON u.id = a.user_id
               WHERE a.reunion_id = $1 AND u.family_unit_id = rfu.family_unit_id
             )"#,
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await?;

    let loc_done: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(DISTINCT rfu.family_unit_id)
           FROM reunion_family_units rfu
           WHERE rfu.reunion_id = $1
             AND EXISTS (
               SELECT 1 FROM location_votes lv
               JOIN location_candidates lc ON lc.id = lv.location_candidate_id
               JOIN users u ON u.id = lv.user_id
               WHERE lc.reunion_id = $1 AND u.family_unit_id = rfu.family_unit_id
             )"#,
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await?;

    let survey_done: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(DISTINCT rfu.family_unit_id)
           FROM reunion_family_units rfu
           WHERE rfu.reunion_id = $1
             AND EXISTS (
               SELECT 1 FROM survey_responses sr
               JOIN survey_questions sq ON sq.id = sr.survey_question_id
               JOIN users u ON u.id = sr.user_id
               WHERE sq.reunion_id = $1 AND u.family_unit_id = rfu.family_unit_id
             )"#,
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await?;

    Ok(Json(SetupProgressResponse {
        availability: ProgressCount { done: avail_done, total },
        locations:    ProgressCount { done: loc_done,   total },
        survey:       ProgressCount { done: survey_done, total },
    }))
}

// ── PUT /reunions/:id/admins/:user_id ─────────────────────────────────────────

pub async fn add_reunion_admin(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    // Only sysadmin or existing RA can add new RAs
    ensure_ra(&user, &state, reunion_id).await?;
    ReunionAdmin::add(state.db(), reunion_id, target_user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── DELETE /reunions/:id/admins/:user_id ──────────────────────────────────────

pub async fn remove_reunion_admin(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    ensure_ra(&user, &state, reunion_id).await?;
    ReunionAdmin::remove(state.db(), reunion_id, target_user_id).await?;
    Ok(StatusCode::NO_CONTENT)
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
