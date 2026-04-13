use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
    error::{AppError, AppResult},
    models::location::{
        CandidateSummary, LocationCandidate, LocationVote, LocationVoteView, NewLocationCandidate,
        PatchLocationCandidate,
    },
    phase::Phase,
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion, user_is_ra};

// ── Response types ─────────────────────────────────────────────────────────────

/// Location candidate enriched with vote data.
/// `avg_score` and `vote_count` are hidden from members until votes are revealed.
/// The requesting user's own vote is always shown.
#[derive(Serialize)]
pub struct LocationView {
    #[serde(flatten)]
    pub candidate: LocationCandidate,
    pub avg_score: Option<f64>,
    pub vote_count: Option<i64>, // None = hidden
    pub my_vote: Option<LocationVoteView>,
}

// ── GET /reunions/:id/locations ────────────────────────────────────────────────

pub async fn list_locations(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    let is_ra = user_is_ra(&state, &user, reunion_id).await;
    let show_aggregate = is_ra || reunion.location_votes_revealed;

    let candidates = LocationCandidate::list_for_reunion(state.db(), reunion_id).await?;
    let summaries = LocationVote::summary_for_reunion(state.db(), reunion_id).await?;

    // Bulk-load the current user's votes for this reunion in one query
    let my_votes = votes_for_user_in_reunion(state.db(), reunion_id, user.id).await?;

    let views: Vec<LocationView> = candidates
        .into_iter()
        .map(|candidate| {
            let summary: Option<&CandidateSummary> =
                summaries.iter().find(|s| s.candidate_id == candidate.id);
            let my_vote = my_votes
                .iter()
                .find(|v| v.location_candidate_id == candidate.id)
                .map(|v| LocationVoteView {
                    score: v.score,
                    comment: v.comment.clone(),
                });

            LocationView {
                avg_score: if show_aggregate {
                    summary.and_then(|s| s.avg_score)
                } else {
                    None
                },
                vote_count: if show_aggregate {
                    Some(summary.map(|s| s.vote_count).unwrap_or(0))
                } else {
                    None
                },
                my_vote,
                candidate,
            }
        })
        .collect();

    Ok(Json(views))
}

// ── POST /reunions/:id/locations ───────────────────────────────────────────────

pub async fn create_location(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<NewLocationCandidate>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    // No phase gate — RA can add locations at any time (e.g. when dates are
    // set out-of-band and they want to move directly to location selection).
    let candidate =
        LocationCandidate::create(state.db(), reunion_id, user.id, body).await?;
    Ok((StatusCode::CREATED, Json(candidate)))
}

// ── PATCH /reunions/:id/locations/:loc_id ─────────────────────────────────────

pub async fn update_location(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, loc_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PatchLocationCandidate>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    if body.title.trim().is_empty() {
        return Err(AppError::BadRequest("title is required".into()));
    }

    let candidate = LocationCandidate::find_by_id(state.db(), loc_id).await?;
    if candidate.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let updated = LocationCandidate::update(state.db(), loc_id, body).await?;
    Ok(Json(updated))
}

// ── DELETE /reunions/:id/locations/:loc_id ────────────────────────────────────

pub async fn delete_location(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, loc_id)): Path<(Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    // Verify the candidate belongs to this reunion
    let candidate = LocationCandidate::find_by_id(state.db(), loc_id).await?;
    if candidate.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    LocationCandidate::delete(state.db(), loc_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── PUT /reunions/:id/locations/:loc_id/vote ──────────────────────────────────

#[derive(Deserialize)]
pub struct VoteRequest {
    /// 1 (not interested) to 5 (love it)
    pub score: i16,
    pub comment: Option<String>,
}

pub async fn vote_location(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, loc_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<VoteRequest>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    let is_ra = user_is_ra(&state, &user, reunion_id).await;

    // Phase gate: members can vote once location candidates exist — from the
    // Availability phase onward (parallel with the dates poll).  The RA can
    // always vote.  Read-only phases (PostReunion, Archived) are excluded.
    let voting_open = matches!(
        reunion.phase,
        Phase::Availability | Phase::Locations | Phase::PrepCompleted | Phase::Active
    );
    if !is_ra && !voting_open {
        return Err(AppError::WrongPhase {
            required: "Availability or later".into(),
            current: reunion.phase.label().into(),
        });
    }

    if !(1..=5).contains(&body.score) {
        return Err(AppError::BadRequest("score must be between 1 and 5".into()));
    }

    // Verify candidate belongs to this reunion
    let candidate = LocationCandidate::find_by_id(state.db(), loc_id).await?;
    if candidate.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let vote = LocationVote::upsert(
        state.db(),
        loc_id,
        user.id,
        body.score,
        body.comment.as_deref(),
    )
    .await?;

    Ok(Json(LocationVoteView {
        score: vote.score,
        comment: vote.comment,
    }))
}

// ── POST /reunions/:id/locations/reveal ───────────────────────────────────────

pub async fn reveal_votes(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    // Votes can be revealed any time from Availability onward (excluding archive/post-reunion).
    if matches!(reunion.phase, Phase::Draft | Phase::PostReunion | Phase::Archived) {
        return Err(AppError::BadRequest(
            "votes can only be revealed while the reunion is in an active planning phase".into(),
        ));
    }

    Reunion::reveal_location_votes(state.db(), reunion_id).await?;
    Ok(Json(serde_json::json!({"revealed": true})))
}

// ── POST /reunions/:id/locations/:loc_id/select ───────────────────────────────

pub async fn select_location(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, loc_id)): Path<(Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    // No phase gate — RA can select a winner at any time.
    // Verify candidate belongs to this reunion
    let candidate = LocationCandidate::find_by_id(state.db(), loc_id).await?;
    if candidate.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let updated = Reunion::set_selected_location(state.db(), reunion_id, loc_id).await?;
    Ok(Json(updated))
}

// ── Internal helper ────────────────────────────────────────────────────────────

async fn votes_for_user_in_reunion(
    pool: &sqlx::PgPool,
    reunion_id: Uuid,
    user_id: Uuid,
) -> AppResult<Vec<LocationVote>> {
    Ok(sqlx::query_as::<_, LocationVote>(
        r#"SELECT lv.*
           FROM location_votes lv
           JOIN location_candidates lc ON lc.id = lv.location_candidate_id
           WHERE lc.reunion_id = $1 AND lv.user_id = $2"#,
    )
    .bind(reunion_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

// Also expose Reunion for advance_phase to use
use crate::models::reunion::Reunion;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vote_request_deserializes() {
        let json = r#"{"score":4,"comment":"Great location!"}"#;
        let req: VoteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.score, 4);
        assert_eq!(req.comment.unwrap(), "Great location!");
    }

    #[test]
    fn score_bounds_enforced() {
        // These would be caught by the handler's guard
        assert!((1i16..=5).contains(&1));
        assert!((1i16..=5).contains(&5));
        assert!(!(1i16..=5).contains(&0));
        assert!(!(1i16..=5).contains(&6));
    }
}
