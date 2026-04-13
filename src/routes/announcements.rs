use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
    error::{AppError, AppResult},
    models::{
        announcement::{Announcement, NewAnnouncement},
        user::User,
    },
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion};

// ── GET /reunions/:id/announcements ───────────────────────────────────────────

pub async fn list_announcements(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    let items = Announcement::list_for_reunion(state.db(), reunion_id).await?;
    Ok(Json(items))
}

// ── POST /reunions/:id/announcements ──────────────────────────────────────────
// RA only.

pub async fn create_announcement(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<NewAnnouncement>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    if body.title.trim().is_empty() {
        return Err(AppError::BadRequest("title cannot be empty".into()));
    }

    let ann = Announcement::create(state.db(), reunion_id, user.id, body).await?;

    // Broadcast announcement emails in the background (non-fatal)
    {
        let state = state.clone();
        let reunion = reunion.clone();
        let ann_title = ann.title.clone();
        let ann_content = ann.content.clone();
        tokio::spawn(async move {
            let users = match User::list_active_verified(state.db()).await {
                Ok(u) => u,
                Err(e) => {
                    tracing::error!("announcement email: failed to load users: {e:?}");
                    return;
                }
            };
            let app_url = &state.config().app_base_url;
            for u in &users {
                if let Err(e) = state
                    .mailer()
                    .send_announcement_email(
                        &u.email,
                        &u.display_name,
                        &reunion.title,
                        &ann_title,
                        &ann_content,
                        app_url,
                    )
                    .await
                {
                    tracing::warn!("announcement email to {}: {e:?}", u.email);
                }
            }
        });
    }

    Ok((StatusCode::CREATED, Json(ann)))
}

// ── DELETE /reunions/:id/announcements/:ann_id ────────────────────────────────
// RA only.

pub async fn delete_announcement(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, ann_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    Announcement::delete(state.db(), ann_id).await?;
    Ok(StatusCode::NO_CONTENT)
}


#[cfg(test)]
mod tests {
    use crate::models::announcement::NewAnnouncement;

    #[test]
    fn new_announcement_deserializes() {
        let json = r#"{"title":"Important Update","content":"Please read this carefully."}"#;
        let req: NewAnnouncement = serde_json::from_str(json).unwrap();
        assert_eq!(req.title, "Important Update");
        assert_eq!(req.content, "Please read this carefully.");
    }

    #[test]
    fn new_announcement_missing_content_fails() {
        let json = r#"{"title":"Oops"}"#;
        let result = serde_json::from_str::<NewAnnouncement>(json);
        assert!(result.is_err());
    }
}
