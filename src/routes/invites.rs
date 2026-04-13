use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
    error::AppResult,
    models::invite::ReunionInvite,
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion};

// ── POST /reunions/:id/invites ─────────────────────────────────────────────────

pub async fn create_invite(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let invite = ReunionInvite::create(state.db(), reunion_id, user.id).await?;

    #[derive(Serialize)]
    struct Resp {
        id: Uuid,
        join_url: String,
    }

    let join_url = format!(
        "{}/join/{}",
        state.config().app_base_url,
        invite.token
    );
    Ok((StatusCode::CREATED, Json(Resp { id: invite.id, join_url })))
}

// ── DELETE /reunions/:id/invites/:invite_id ────────────────────────────────────

pub async fn revoke_invite(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, invite_id)): Path<(Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    ReunionInvite::deactivate(state.db(), invite_id, reunion_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── DELETE /reunions/:id/invite-members/:user_id ───────────────────────────────

pub async fn remove_invite_member(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    ReunionInvite::remove_member(state.db(), reunion_id, target_user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
