use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{
        location::LocationCandidate,
        reunion::{Reunion, ReunionAdmin, ReunionDate, ReunionFamilyUnit},
        user::User,
    },
    phase::Phase,
    state::AppState,
};

/// Load a reunion by ID or return 404.
pub async fn load_reunion(state: &AppState, id: Uuid) -> AppResult<Reunion> {
    Reunion::find_by_id(state.db(), id).await
}

/// Returns true if the user is a sysadmin or listed as an RA for this reunion.
pub async fn user_is_ra(state: &AppState, user: &User, reunion_id: Uuid) -> bool {
    if user.is_sysadmin() { return true; }
    ReunionAdmin::list_ids_for_reunion(state.db(), reunion_id)
        .await
        .map(|ids| ids.contains(&user.id))
        .unwrap_or(false)
}

/// Returns Forbidden if the user is neither a sysadmin nor an RA for this reunion.
pub async fn ensure_ra(user: &User, state: &AppState, reunion_id: Uuid) -> AppResult<()> {
    if user_is_ra(state, user, reunion_id).await {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Returns true if the user may access this reunion at member level:
/// - Sysadmins always may.
/// - RAs always may (any phase, including Draft).
/// - Draft phase: RA/sysadmin only.
/// - Other phases: any user whose family unit is enrolled, or who joined via invite link.
pub async fn user_is_reunion_member(state: &AppState, user: &User, reunion: &Reunion) -> bool {
    if user.is_sysadmin() { return true; }
    if user_is_ra(state, user, reunion.id).await { return true; }
    if reunion.phase == Phase::Draft { return false; }
    if let Some(fu_id) = user.family_unit_id {
        if ReunionFamilyUnit::list_ids_for_reunion(state.db(), reunion.id)
            .await
            .map(|ids| ids.contains(&fu_id))
            .unwrap_or(false)
        {
            return true;
        }
    }
    // Also allow users who joined via an invite link (reunion_invite_members).
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM reunion_invite_members WHERE reunion_id = $1 AND user_id = $2)",
    )
    .bind(reunion.id)
    .bind(user.id)
    .fetch_one(state.db())
    .await
    .unwrap_or(false)
}

/// Returns the IANA timezone string for the reunion's selected location, or "UTC".
pub async fn get_reunion_tz_string(state: &AppState, reunion: &Reunion) -> String {
    if let Some(loc_id) = reunion.selected_location_id {
        if let Ok(loc) = LocationCandidate::find_by_id(state.db(), loc_id).await {
            return loc.timezone;
        }
    }
    "UTC".to_owned()
}

/// If the reunion is in `PrepCompleted` phase and the reunion start date has arrived
/// (evaluated in the location's timezone), auto-advances it to `Active`.
/// Returns the updated `Reunion` if advanced, `None` otherwise.
pub async fn maybe_auto_activate(state: &AppState, reunion: &Reunion) -> Option<Reunion> {
    if reunion.phase != Phase::PrepCompleted {
        return None;
    }
    let rd = ReunionDate::find_for_reunion(state.db(), reunion.id)
        .await
        .ok()
        .flatten()?;
    let tz_str = get_reunion_tz_string(state, reunion).await;
    let tz: chrono_tz::Tz = tz_str.parse().unwrap_or(chrono_tz::UTC);
    let today = chrono::Utc::now().with_timezone(&tz).date_naive();
    if today >= rd.start_date {
        Reunion::advance_phase(state.db(), reunion.id, &Phase::PrepCompleted)
            .await
            .ok()
    } else {
        None
    }
}

/// Load a reunion and verify the user has member-level access. For use in page handlers.
/// Returns `Err(Redirect::to("/dashboard"))` on access denial.
pub async fn load_reunion_for_member(
    state: &AppState,
    user: &User,
    reunion_id: Uuid,
) -> Result<Reunion, Response> {
    let reunion = Reunion::find_by_id(state.db(), reunion_id)
        .await
        .map_err(|_| Redirect::to("/dashboard").into_response())?;
    if user_is_reunion_member(state, user, &reunion).await {
        Ok(reunion)
    } else {
        Err(Redirect::to("/dashboard").into_response())
    }
}
