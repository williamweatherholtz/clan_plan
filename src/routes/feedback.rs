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
    models::feedback::{Feedback, NewSurveyQuestion, SurveyQuestion, SurveyResponse},
    phase::Phase,
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion};

// ── GET /reunions/:id/feedback ────────────────────────────────────────────────
// RA only — members submit anonymously (or at least, not broadcasted to them).

pub async fn list_feedback(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let items = Feedback::list_for_reunion(state.db(), reunion_id).await?;
    Ok(Json(items))
}

// ── POST /reunions/:id/feedback ───────────────────────────────────────────────
// Any member, active phase onward.

#[derive(Deserialize)]
pub struct CreateFeedbackRequest {
    pub content: String,
}

pub async fn create_feedback(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<CreateFeedbackRequest>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;

    if !matches!(reunion.phase, Phase::Active | Phase::PostReunion) {
        return Err(AppError::WrongPhase {
            required: "active or post_reunion".into(),
            current: reunion.phase.label().into(),
        });
    }

    if body.content.trim().is_empty() {
        return Err(AppError::BadRequest("content cannot be empty".into()));
    }

    let item = Feedback::create(state.db(), reunion_id, user.id, body.content.trim()).await?;
    Ok((StatusCode::CREATED, Json(item)))
}

// ── GET /reunions/:id/survey/questions ────────────────────────────────────────
// Visible to all members in post_reunion phase.

pub async fn list_survey_questions(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    let questions = SurveyQuestion::list_for_reunion(state.db(), reunion_id).await?;
    Ok(Json(questions))
}

// ── POST /reunions/:id/survey/questions ───────────────────────────────────────
// RA only.

pub async fn create_survey_question(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<NewSurveyQuestion>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    if body.question_text.trim().is_empty() {
        return Err(AppError::BadRequest("question_text cannot be empty".into()));
    }

    let q = SurveyQuestion::create(state.db(), reunion_id, body).await?;
    Ok((StatusCode::CREATED, Json(q)))
}

// ── DELETE /reunions/:id/survey/questions/:q_id ───────────────────────────────
// RA only.

pub async fn delete_survey_question(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, q_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    SurveyQuestion::delete(state.db(), q_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── POST /reunions/:id/survey/questions/:q_id/responses ───────────────────────
// Any member — appends a new response to a survey question (no phase gate).

#[derive(Deserialize)]
pub struct SurveyResponseRequest {
    pub response_text: String,
}

pub async fn create_survey_response(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, q_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SurveyResponseRequest>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    if body.response_text.trim().is_empty() {
        return Err(AppError::BadRequest("response_text cannot be empty".into()));
    }

    let response =
        SurveyResponse::create(state.db(), q_id, user.id, body.response_text.trim()).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

// ── PATCH /reunions/:id/survey/questions/:q_id/responses/:r_id ────────────────
// Owner only — edit the text of an existing response.

pub async fn update_survey_response(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, _q_id, r_id)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<SurveyResponseRequest>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    if body.response_text.trim().is_empty() {
        return Err(AppError::BadRequest("response_text cannot be empty".into()));
    }

    let updated =
        SurveyResponse::update(state.db(), r_id, user.id, body.response_text.trim()).await?;

    match updated {
        Some(r) => Ok(Json(r).into_response()),
        None => Err(AppError::Forbidden),
    }
}

// ── DELETE /reunions/:id/survey/questions/:q_id/responses/:r_id ───────────────
// Owner only — delete one of their own responses.

pub async fn delete_survey_response(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, _q_id, r_id)): Path<(Uuid, Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;

    let deleted = SurveyResponse::delete(state.db(), r_id, user.id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::Forbidden)
    }
}

// ── GET /reunions/:id/survey/responses ────────────────────────────────────────
// RA only — returns all responses grouped by question.

#[derive(Serialize)]
pub struct QuestionWithResponses {
    #[serde(flatten)]
    pub question: SurveyQuestion,
    pub responses: Vec<SurveyResponse>,
}

pub async fn list_survey_responses(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let questions = SurveyQuestion::list_for_reunion(state.db(), reunion_id).await?;
    let all_responses = SurveyResponse::list_for_reunion(state.db(), reunion_id).await?;

    let result: Vec<QuestionWithResponses> = questions
        .into_iter()
        .map(|q| {
            let responses = all_responses
                .iter()
                .filter(|r| r.survey_question_id == q.id)
                .cloned()
                .collect();
            QuestionWithResponses {
                question: q,
                responses,
            }
        })
        .collect();

    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_request_deserializes() {
        let json = r#"{"content":"Great reunion!"}"#;
        let req: CreateFeedbackRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.content, "Great reunion!");
    }

    #[test]
    fn survey_response_deserializes() {
        let json = r#"{"response_text":"It was wonderful"}"#;
        let req: SurveyResponseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.response_text, "It was wonderful");
    }

    #[test]
    fn survey_question_deserializes() {
        let json = r#"{"question_text":"What went well?","order_index":0}"#;
        let req: NewSurveyQuestion = serde_json::from_str(json).unwrap();
        assert_eq!(req.question_text, "What went well?");
        assert_eq!(req.order_index, 0);
    }
}
