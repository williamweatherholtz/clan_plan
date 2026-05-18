use axum::{
    extract::{Path, Query, State},
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
        ActivityComment, ActivityIdea, ActivityStatus,
        NewActivityIdea, PatchActivityIdea,
    },
    state::AppState,
};

use super::helpers::{load_reunion, user_is_ra};

// ── Response types ─────────────────────────────────────────────────────────────

/// Idea enriched with aggregate counts.
#[derive(Serialize)]
pub struct ActivityIdeaView {
    #[serde(flatten)]
    pub idea: ActivityIdea,
    pub comment_count: i64,
}

// ── GET /reunions/:id/activities ──────────────────────────────────────────────

pub async fn list_activities(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let ideas = ActivityIdea::list_for_reunion(state.db(), reunion_id).await?;
    let summaries = ActivityIdea::summaries_for_reunion(state.db(), reunion_id).await?;

    let views: Vec<ActivityIdeaView> = ideas
        .into_iter()
        .map(|idea| {
            let summary = summaries.iter().find(|s| s.idea_id == idea.id);
            ActivityIdeaView {
                comment_count: summary.map(|s| s.comment_count).unwrap_or(0),
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

// ── PATCH /reunions/:id/activities/:act_id ───────────────────────────────────
// Proposer or RA can edit title, description, category, needs_time_slot, and
// suggested_time. Status-locked activities (scheduled/cancelled) can still be
// edited — the server doesn't block it; the UI is just less likely to show it.

pub async fn update_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PatchActivityIdea>,
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
    if !["group", "optional", "meal"].contains(&body.category.as_str()) {
        return Err(AppError::BadRequest("category must be group, optional, or meal".into()));
    }

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }
    // Only the proposer may edit. RAs/sysadmins can pin, cancel, schedule, or
    // delete an idea via other endpoints, but they may not silently rewrite
    // someone else's wording.
    if idea.proposed_by != user.id {
        return Err(AppError::Forbidden);
    }

    let updated = ActivityIdea::update(state.db(), act_id, &body).await?;
    Ok(Json(updated))
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

// ── PATCH /reunions/:id/activities/:act_id/comments/:cmt_id ───────────────────
// Only the comment's original author may edit. RAs/sysadmins can delete but
// cannot silently rewrite someone else's words.

pub async fn update_comment(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, _act_id, cmt_id)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<CommentRequest>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    if body.content.trim().is_empty() {
        return Err(AppError::BadRequest("comment cannot be empty".into()));
    }
    if body.content.len() > 2_000 {
        return Err(AppError::BadRequest(
            "comment cannot exceed 2,000 characters".into(),
        ));
    }

    let updated =
        ActivityComment::update(state.db(), cmt_id, user.id, body.content.trim()).await?;
    Ok(Json(updated))
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

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let is_author = idea.proposed_by == user.id;
    let is_ra = user_is_ra(&state, &user, reunion_id).await;

    // Per-status authorisation:
    //   Pinned    — RA only (curatorial highlight)
    //   Cancelled — author only (only the proposer can withdraw their idea)
    //   Proposed  — author or RA (un-cancel by author, un-pin by RA)
    //   Scheduled — never set directly; goes through the promote endpoint
    match body.status {
        ActivityStatus::Pinned => {
            if !is_ra {
                return Err(AppError::Forbidden);
            }
        }
        ActivityStatus::Cancelled => {
            if !is_author {
                return Err(AppError::Forbidden);
            }
        }
        ActivityStatus::Proposed => {
            if !is_author && !is_ra {
                return Err(AppError::Forbidden);
            }
        }
        ActivityStatus::Scheduled => {
            return Err(AppError::BadRequest(
                "use the promote endpoint to schedule an activity".into(),
            ));
        }
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

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    // Author or RA only — no one else can attach an idea to the schedule.
    let is_author = idea.proposed_by == user.id;
    let is_ra = user_is_ra(&state, &user, reunion_id).await;
    if !is_author && !is_ra {
        return Err(AppError::Forbidden);
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
// Author-only. RAs may pin/cancel/promote but not delete someone else's idea.
// If the idea was promoted to a schedule block, that block is deleted too.

pub async fn delete_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }
    if idea.proposed_by != user.id {
        return Err(AppError::Forbidden);
    }

    // Capture before deletion — the promoted block should go with the idea.
    let promoted_block_id = idea.promoted_to_block_id;

    // is_admin=false means the model's WHERE user_id=$2 clause is used,
    // which double-checks the same author rule at the SQL level.
    ActivityIdea::delete(state.db(), act_id, user.id, false).await?;

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

/// Comment enriched with an `is_mine` flag so the UI can offer edit/delete on
/// the requesting user's own comments without needing the user's UUID up front.
#[derive(Serialize)]
pub struct CommentResponseView {
    pub id: Uuid,
    pub activity_idea_id: Uuid,
    pub user_id: Uuid,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub display_name: String,
    pub is_mine: bool,
}

pub async fn list_comments(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let comments = ActivityComment::list_with_names(state.db(), act_id).await?;
    let enriched: Vec<CommentResponseView> = comments
        .into_iter()
        .map(|c| CommentResponseView {
            is_mine: c.user_id == user.id,
            id: c.id,
            activity_idea_id: c.activity_idea_id,
            user_id: c.user_id,
            content: c.content,
            created_at: c.created_at,
            display_name: c.display_name,
        })
        .collect();
    Ok(Json(enriched))
}

// ── PUT/DELETE /reunions/:id/activities/:act_id/rsvp ──────────────────────────
//
// Optional `?role=` query: "in" (default), "make", "cleanup". The make/cleanup
// roles are only valid for activity ideas with category="meal".

#[derive(Deserialize, Default)]
pub struct RsvpQuery {
    #[serde(default)]
    pub role: Option<String>,
}

fn validate_rsvp_role(role: &str, idea: &ActivityIdea) -> AppResult<()> {
    match role {
        "in" => Ok(()),
        "make" | "cleanup" => {
            if idea.category == "meal" {
                Ok(())
            } else {
                Err(AppError::BadRequest(
                    "make/cleanup roles only apply to meal activities".into(),
                ))
            }
        }
        _ => Err(AppError::BadRequest("invalid role".into())),
    }
}

pub async fn rsvp_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<RsvpQuery>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    let idea = ActivityIdea::find_by_id(state.db(), act_id).await?;
    if idea.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }
    let role = q.role.as_deref().unwrap_or("in");
    validate_rsvp_role(role, &idea)?;
    sqlx::query(
        "INSERT INTO activity_rsvps (activity_idea_id, user_id, role)
         VALUES ($1, $2, $3)
         ON CONFLICT (activity_idea_id, user_id, role) DO NOTHING",
    )
    .bind(act_id)
    .bind(user.id)
    .bind(role)
    .execute(state.db())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unrsvp_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, act_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<RsvpQuery>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    let role = q.role.as_deref().unwrap_or("in");
    if !matches!(role, "in" | "make" | "cleanup") {
        return Err(AppError::BadRequest("invalid role".into()));
    }
    sqlx::query(
        "DELETE FROM activity_rsvps WHERE activity_idea_id = $1 AND user_id = $2 AND role = $3",
    )
    .bind(act_id)
    .bind(user.id)
    .bind(role)
    .execute(state.db())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

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
