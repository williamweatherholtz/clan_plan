use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use chrono::NaiveDate;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
    error::{AppError, AppResult},
    models::availability::Availability,
    phase::Phase,
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion};

// ── GET /reunions/:id/availability/me ─────────────────────────────────────────

pub async fn get_my_availability(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    // Any member can read their own availability at any phase
    load_reunion(&state, reunion_id).await?;
    let dates = Availability::for_user(state.db(), reunion_id, user.id).await?;
    Ok(Json(dates))
}

// ── PUT /reunions/:id/availability/me ─────────────────────────────────────────

#[derive(Deserialize)]
pub struct SetAvailabilityRequest {
    /// Full replacement: all dates the member is available.
    /// Pass an empty array to clear all availability.
    pub dates: Vec<NaiveDate>,
}

pub async fn set_my_availability(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<SetAvailabilityRequest>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;

    if reunion.phase != Phase::Availability {
        return Err(AppError::WrongPhase {
            required: Phase::Availability.label().into(),
            current: reunion.phase.label().into(),
        });
    }

    let rows = Availability::replace(state.db(), reunion_id, user.id, &body.dates).await?;
    Ok(Json(rows))
}

// ── GET /reunions/:id/availability/heatmap ────────────────────────────────────

pub async fn get_heatmap(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let heatmap = Availability::heatmap(state.db(), reunion_id).await?;
    let respondent_count = Availability::respondent_count(state.db(), reunion_id).await?;

    #[derive(serde::Serialize)]
    struct HeatmapResponse {
        heatmap: Vec<crate::models::availability::HeatmapEntry>,
        respondent_count: i64,
    }

    Ok(Json(HeatmapResponse {
        heatmap,
        respondent_count,
    }))
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn set_availability_request_deserializes() {
        let json = r#"{"dates":["2026-07-10","2026-07-11","2026-07-14"]}"#;
        let req: SetAvailabilityRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.dates.len(), 3);
        assert_eq!(
            req.dates[0],
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()
        );
    }

    #[test]
    fn empty_availability_deserializes() {
        let json = r#"{"dates":[]}"#;
        let req: SetAvailabilityRequest = serde_json::from_str(json).unwrap();
        assert!(req.dates.is_empty());
    }
}
