//! Routes for the per-reunion rules pane.
//!
//! - `PATCH /api/reunions/:id/rules`            — RA-only, update label/body
//! - `POST  /api/reunions/:id/rules/comments`   — post a comment (any member)
//! - `DELETE /api/reunions/:id/rules/comments/:cmt_id` — author or RA

use askama::Template;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
    error::{AppError, AppResult},
    models::{reunion::Reunion, rules::{RulesComment, RulesCommentView}},
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion_for_api_member, user_is_ra};

// ── PATCH /api/reunions/:id/rules ─────────────────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct UpdateRulesRequest {
    /// New label for the tab/pane. Leave None to keep the current one.
    /// Trimmed; rejected if non-None and empty/whitespace after trim, since
    /// a blank tab label would render as an unclickable strip in the nav.
    pub label: Option<String>,
    /// New body. Leave None to keep the current one. An empty string clears
    /// the body back to NULL (caller intent: "delete my rules doc").
    pub body: Option<String>,
}

pub async fn update_rules(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(req): Json<UpdateRulesRequest>,
) -> AppResult<Json<Reunion>> {
    load_reunion_for_api_member(&state, &user, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let label = match req.label.as_deref().map(str::trim) {
        Some("") => return Err(AppError::BadRequest("label cannot be blank".into())),
        Some(s) if s.len() > 80 => {
            return Err(AppError::BadRequest("label cannot exceed 80 characters".into()))
        }
        other => other,
    };

    // Body length cap protects the markdown renderer from a pathologically
    // large input. 64 KB is generous for a house-rules doc.
    if let Some(b) = req.body.as_deref() {
        if b.len() > 64 * 1024 {
            return Err(AppError::BadRequest("body cannot exceed 64 KB".into()));
        }
    }

    let updated = Reunion::update_rules(
        state.db(),
        reunion_id,
        label,
        req.body.as_deref(),
    )
    .await?;
    Ok(Json(updated))
}

// ── POST /api/reunions/:id/rules/comments ─────────────────────────────────────

#[derive(Deserialize)]
pub struct CommentRequest {
    pub content: String,
}

#[derive(Template)]
#[template(path = "partials/rules_comments_list.html")]
struct RulesCommentsListPartial<'a> {
    reunion_id: Uuid,
    comments: &'a [RulesCommentViewMine],
    current_user_id: Uuid,
}

/// Enriched view passed to the partial — adds `is_mine` so the template
/// can show the delete button without a per-row comparison call.
pub struct RulesCommentViewMine {
    pub id: Uuid,
    pub author_name: String,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub is_mine: bool,
}

fn is_htmx_request(headers: &HeaderMap) -> bool {
    headers
        .get("HX-Request")
        .and_then(|v| v.to_str().ok())
        == Some("true")
}

pub async fn create_comment(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> AppResult<Response> {
    load_reunion_for_api_member(&state, &user, reunion_id).await?;

    // Accept JSON OR form-encoded body (matches the activities comment flow,
    // which lets the htmx <form hx-post> just work without an extension).
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let req: CommentRequest = if ct.starts_with("application/json") {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid JSON: {e}").into()))?
    } else {
        serde_urlencoded::from_bytes(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid form body: {e}").into()))?
    };

    let trimmed = req.content.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("comment cannot be empty".into()));
    }
    if trimmed.len() > 2_000 {
        return Err(AppError::BadRequest(
            "comment cannot exceed 2,000 characters".into(),
        ));
    }

    let _comment = RulesComment::create(state.db(), reunion_id, user.id, trimmed).await?;

    if is_htmx_request(&headers) {
        let enriched = enriched_comments(&state, reunion_id, user.id).await?;
        let tpl = RulesCommentsListPartial {
            reunion_id,
            comments: &enriched,
            current_user_id: user.id,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("rules comments render: {e}")))?;
        return Ok(Html(html).into_response());
    }

    Ok((StatusCode::CREATED).into_response())
}

// ── DELETE /api/reunions/:id/rules/comments/:cmt_id ───────────────────────────

pub async fn delete_comment(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, cmt_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion_for_api_member(&state, &user, reunion_id).await?;

    let comment = RulesComment::find_by_id(state.db(), cmt_id).await?;
    if comment.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let is_admin = user_is_ra(&state, &user, reunion_id).await;
    if comment.user_id != user.id && !is_admin {
        return Err(AppError::Forbidden);
    }

    RulesComment::delete(state.db(), cmt_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub async fn enriched_comments(
    state: &AppState,
    reunion_id: Uuid,
    current_user_id: Uuid,
) -> AppResult<Vec<RulesCommentViewMine>> {
    let comments = RulesComment::list_for_reunion(state.db(), reunion_id).await?;
    Ok(comments
        .into_iter()
        .map(|c: RulesCommentView| RulesCommentViewMine {
            is_mine: c.user_id == current_user_id,
            id: c.id,
            author_name: c.author_name,
            content: c.content,
            created_at: c.created_at,
        })
        .collect())
}

/// Render the user-supplied markdown to safe HTML. Default `markdown` crate
/// options disable raw HTML pass-through and reject `javascript:` URLs.
/// Empty / whitespace-only input returns the empty string so callers can
/// emit a "no rules yet" empty state.
pub fn render_markdown(body: Option<&str>) -> String {
    let raw = body.unwrap_or("").trim();
    if raw.is_empty() {
        return String::new();
    }
    markdown::to_html(raw)
}

/// Alias used by pages::rules_page so the dependency on this module is
/// self-documenting at the call site.
pub use enriched_comments as enriched_comments_for_render;
