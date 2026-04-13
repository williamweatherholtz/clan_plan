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
    models::activity::{
        ActivityComment, ActivityIdea, ActivityStatus, ActivityVote,
        NewActivityIdea,
    },
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion, user_is_ra};

// ── Response types ─────────────────────────────────────────────────────────────

/// Idea enriched with aggregate interest data + the requesting user's vote.
#[derive(Serialize)]
pub struct ActivityIdeaView {
    #[serde(flatten)]
    pub idea: ActivityIdea,
    pub avg_interest: Option<f64>,
    pub vote_count: i64,
    pub comment_count: i64,
    pub my_vote: Option<i16>,
}

// ── GET /reunions/:id/activities ──────────────────────────────────────────────

pub async fn list_activities(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let ideas = ActivityIdea::list_for_reunion(state.db(), reunion_id).await?;
    let summaries = ActivityIdea::summaries_for_reunion(state.db(), reunion_id).await?;
    let my_votes = my_votes_for_reunion(state.db(), reunion_id, user.id).await?;

    let views: Vec<ActivityIdeaView> = ideas
        .into_iter()
        .map(|idea| {
            let summary = summaries.iter().find(|s| s.idea_id == idea.id);
            let my_vote = my_votes
                .iter()
                .find(|v| v.activity_idea_id == idea.id)
                .map(|v| v.interest_score);

            ActivityIdeaView {
                avg_interest: summary.and_then(|s| s.avg_interest),
                vote_count: summary.map(|s| s.vote_count).unwrap_or(0),
                comment_count: summary.map(|s| s.comment_count).unwrap_or(0),
                my_vote,
                idea,
            }
        })
        .collect();

    Ok(Json(views))
}

// ── POST /reunions/:id/activities ─────────────────────────────────────────────
// Not phase-gated — any member can propose an idea at any time.

pub async fn create_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<NewActivityIdea>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    if body.title.trim().is_empty() {
        return Err(AppError::BadRequest("title cannot be empty".into()));
    }
    if body.title.len() > 200 {
        return Err(AppError::BadRequest("title cannot exceed 200 characters".into()));
    }
    if body.description.as_deref().map(|d| d.len()).unwrap_or(0) > 5_000 {
        return Err(AppError::BadRequest("description cannot exceed 5,000 characters".into()));
    }

    let idea = ActivityIdea::create(state.db(), reunion_id, user.id, body).await?;
    Ok((StatusCode::CREATED, Json(idea)))
}

// ── PUT /reunions/:id/activities/:act_id/vote ─────────────────────────────────

#[derive(Deserialize)]
pub struct VoteRequest {
    /// 1 (not interested) to 5 (absolutely!)
    pub interest_score: i16,
}

pub async fn vote_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<VoteRequest>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    if !(1..=5).contains(&body.interest_score) {
        return Err(AppError::BadRequest(
            "interest_score must be between 1 and 5".into(),
        ));
    }

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let vote = ActivityVote::upsert(state.db(), act_id, user.id, body.interest_score).await?;
    Ok(Json(vote))
}

// ── POST /reunions/:id/activities/:act_id/comments ────────────────────────────

#[derive(Deserialize)]
pub struct CommentRequest {
    pub content: String,
}

pub async fn create_comment(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<CommentRequest>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    if body.content.trim().is_empty() {
        return Err(AppError::BadRequest("comment cannot be empty".into()));
    }
    if body.content.len() > 2_000 {
        return Err(AppError::BadRequest("comment cannot exceed 2,000 characters".into()));
    }

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let comment =
        ActivityComment::create(state.db(), act_id, user.id, body.content.trim()).await?;
    Ok((StatusCode::CREATED, Json(comment)))
}

// ── DELETE /reunions/:id/activities/:act_id/comments/:cmt_id ─────────────────

pub async fn delete_comment(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, _act_id, cmt_id)): Path<(Uuid, Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    let is_admin = user_is_ra(&state, &user, reunion_id).await;

    ActivityComment::delete(state.db(), cmt_id, user.id, is_admin).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── PATCH /reunions/:id/activities/:act_id/status ─────────────────────────────

#[derive(Deserialize)]
pub struct SetStatusRequest {
    pub status: ActivityStatus,
}

pub async fn set_status(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SetStatusRequest>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let updated = ActivityIdea::set_status(state.db(), act_id, &body.status).await?;
    Ok(Json(updated))
}

// ── POST /reunions/:id/activities/:act_id/promote ─────────────────────────────

#[derive(Deserialize)]
pub struct PromoteRequest {
    /// ID of an existing schedule block to link this idea to.
    pub schedule_block_id: Uuid,
}

pub async fn promote_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PromoteRequest>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    // Verify the block belongs to this reunion
    let block =
        crate::models::schedule::ScheduleBlock::find_by_id(state.db(), body.schedule_block_id)
            .await?;
    if block.reunion_id != reunion_id {
        return Err(AppError::BadRequest(
            "schedule block does not belong to this reunion".into(),
        ));
    }

    let updated =
        ActivityIdea::promote_to_block(state.db(), act_id, body.schedule_block_id).await?;
    Ok(Json(updated))
}

// ── DELETE /reunions/:id/activities/:act_id ───────────────────────────────────
// Allowed by the original proposer or any RA.
// If the idea was promoted to a schedule block, that block is deleted too.

pub async fn delete_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    let is_admin = user_is_ra(&state, &user, reunion_id).await;

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    // Capture before deletion — the promoted block should go with the idea.
    let promoted_block_id = idea.promoted_to_block_id;

    ActivityIdea::delete(state.db(), act_id, user.id, is_admin).await?;

    // If this idea was promoted to a schedule block, delete that block too.
    // Schedule slots cascade automatically via the schedule_blocks FK.
    if let Some(block_id) = promoted_block_id {
        sqlx::query("DELETE FROM schedule_blocks WHERE id = $1")
            .bind(block_id)
            .execute(state.db())
            .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── GET /reunions/:id/activities/:act_id/comments ─────────────────────────────

pub async fn list_comments(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let comments = ActivityComment::list_with_names(state.db(), act_id).await?;
    Ok(Json(comments))
}

// ── PUT /reunions/:id/activities/:act_id/rsvp ─────────────────────────────────

pub async fn rsvp_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }
    sqlx::query(
        "INSERT INTO activity_rsvps (activity_idea_id, user_id)
         VALUES ($1, $2)
         ON CONFLICT (activity_idea_id, user_id) DO NOTHING",
    )
    .bind(act_id)
    .bind(user.id)
    .execute(state.db())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── DELETE /reunions/:id/activities/:act_id/rsvp ──────────────────────────────

pub async fn unrsvp_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    sqlx::query(
        "DELETE FROM activity_rsvps WHERE activity_idea_id = $1 AND user_id = $2",
    )
    .bind(act_id)
    .bind(user.id)
    .execute(state.db())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Internal helper ────────────────────────────────────────────────────────────

async fn my_votes_for_reunion(
    pool: &sqlx::PgPool,
    reunion_id: Uuid,
    user_id: Uuid,
) -> AppResult<Vec<ActivityVote>> {
    Ok(sqlx::query_as::<_, ActivityVote>(
        r#"SELECT av.*
           FROM activity_votes av
           JOIN activity_ideas ai ON ai.id = av.activity_idea_id
           WHERE ai.reunion_id = $1 AND av.user_id = $2"#,
    )
    .bind(reunion_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vote_request_deserializes() {
        let json = r#"{"interest_score":5}"#;
        let req: VoteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.interest_score, 5);
    }

    #[test]
    fn promote_request_deserializes() {
        let id = Uuid::new_v4();
        let json = format!(r#"{{"schedule_block_id":"{id}"}}"#);
        let req: PromoteRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req.schedule_block_id, id);
    }

    #[test]
    fn set_status_request_deserializes() {
        let json = r#"{"status":"pinned"}"#;
        let req: SetStatusRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.status, ActivityStatus::Pinned);
    }
}
