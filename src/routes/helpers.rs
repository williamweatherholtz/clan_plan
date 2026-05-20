use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts, Path},
    http::request::Parts,
    response::{IntoResponse, Redirect, Response},
};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
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
///
/// **This does NOT authorize.** It just resolves the UUID to a row. Use
/// `load_reunion_for_api_member` (JSON routes) or `load_reunion_for_member`
/// (HTML page routes) at the route boundary to gate access. The bare
/// `load_reunion` only remains for the handful of helpers that legitimately
/// need a Reunion without an associated user (e.g. background
/// `maybe_auto_activate`).
pub async fn load_reunion(state: &AppState, id: Uuid) -> AppResult<Reunion> {
    Reunion::find_by_id(state.db(), id).await
}

/// Load a reunion and verify the user has member-level access. For use in
/// JSON API handlers — returns `AppError::Forbidden` on access denial (HTML
/// page handlers should use `load_reunion_for_member` instead, which
/// redirects).
pub async fn load_reunion_for_api_member(
    state: &AppState,
    user: &User,
    reunion_id: Uuid,
) -> AppResult<Reunion> {
    let reunion = Reunion::find_by_id(state.db(), reunion_id).await?;
    if !user_is_reunion_member(state, user, &reunion).await {
        return Err(AppError::Forbidden);
    }
    Ok(reunion)
}

// ── Authorization extractors ────────────────────────────────────────────────
//
// `ReunionMember` resolves the `:id` path param to a Reunion, loads the
// CurrentUser, and rejects with Forbidden if the user is not a member.
// `ReunionRa` layers an RA check on top. Use one of these in every route
// handler under `/api/reunions/:id/...` — never `load_reunion(...).await?`
// at the route boundary alone.

/// Member-level access to a reunion. The `:id` path param is read by name
/// (so this composes with handlers that take additional `Path<(...)>` for
/// nested IDs like `:act_id` / `:exp_id`).
pub struct ReunionMember {
    pub user: User,
    pub reunion: Reunion,
    pub is_ra: bool,
}

async fn reunion_id_from_path<S>(parts: &mut Parts, state: &S) -> AppResult<Uuid>
where
    S: Send + Sync,
{
    let Path(params): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
        .await
        .map_err(|_| AppError::BadRequest("missing path params".into()))?;
    let id_str = params
        .get("id")
        .ok_or_else(|| AppError::BadRequest("missing :id path param".into()))?;
    id_str
        .parse::<Uuid>()
        .map_err(|_| AppError::BadRequest("invalid uuid in :id path param".into()))
}

#[async_trait]
impl<S> FromRequestParts<S> for ReunionMember
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;
        let app_state = AppState::from_ref(state);

        let reunion_id = reunion_id_from_path(parts, state).await?;
        let reunion = Reunion::find_by_id(app_state.db(), reunion_id).await?;

        if !user_is_reunion_member(&app_state, &user, &reunion).await {
            return Err(AppError::Forbidden);
        }

        let is_ra = user_is_ra(&app_state, &user, reunion.id).await;

        Ok(ReunionMember { user, reunion, is_ra })
    }
}

/// RA-or-sysadmin access to a reunion. Wraps `ReunionMember` with an extra
/// `is_ra` gate. Use `let ReunionRa(ctx) = ...;` to access the inner user
/// and reunion.
pub struct ReunionRa(pub ReunionMember);

#[async_trait]
impl<S> FromRequestParts<S> for ReunionRa
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let member = ReunionMember::from_request_parts(parts, state).await?;
        if !member.is_ra {
            return Err(AppError::Forbidden);
        }
        Ok(ReunionRa(member))
    }
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

/// Returns Forbidden if the user is not a member of this reunion (per `user_is_reunion_member`).
pub async fn ensure_member(user: &User, state: &AppState, reunion: &Reunion) -> AppResult<()> {
    if user_is_reunion_member(state, user, reunion).await {
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

/// For a scheduled block, return `(make_names, cleanup_names)` from the
/// activity_rsvps of any non-cancelled meal idea promoted into this block.
/// Used by the schedule page and the .ics export to surface make/cleanup
/// commitments alongside the block (the buttons themselves live on the
/// activities page; this is read-only display).
///
/// Returns empty vectors when the block has no linked meal idea or when
/// no one has signed up — callers can `.is_empty()` to suppress rendering.
pub async fn meal_rsvp_names_for_block(
    pool: &PgPool,
    block_id: Uuid,
) -> (Vec<String>, Vec<String>) {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT u.display_name, ar.role
         FROM activity_rsvps ar
         JOIN users u ON u.id = ar.user_id
         JOIN activity_ideas ai ON ai.id = ar.activity_idea_id
         WHERE ai.promoted_to_block_id = $1
           AND ai.category = 'meal'
           AND ai.status != 'cancelled'
           AND ar.role IN ('make', 'cleanup')
         ORDER BY u.display_name",
    )
    .bind(block_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let make = rows
        .iter()
        .filter(|(_, r)| r == "make")
        .map(|(n, _)| n.clone())
        .collect();
    let cleanup = rows
        .iter()
        .filter(|(_, r)| r == "cleanup")
        .map(|(n, _)| n.clone())
        .collect();
    (make, cleanup)
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
