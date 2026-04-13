use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use serde::Deserialize;

use crate::{
    auth::session::CurrentUser,
    error::{AppError, AppResult},
    models::schedule::{
        NewScheduleBlock, NewSignupSlot, ScheduleBlock, Signup, SignupSlot,
    },
    phase::Phase,
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion};

// ── Response types ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SlotWithSignups {
    #[serde(flatten)]
    pub slot: SignupSlot,
    pub signups: Vec<Signup>,
    pub is_full: bool,
}

#[derive(Serialize)]
pub struct BlockWithSlots {
    #[serde(flatten)]
    pub block: ScheduleBlock,
    pub slots: Vec<SlotWithSignups>,
}

// ── GET /reunions/:id/schedule ─────────────────────────────────────────────────

pub async fn get_schedule(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let blocks = ScheduleBlock::list_for_reunion(state.db(), reunion_id).await?;
    let mut result: Vec<BlockWithSlots> = Vec::with_capacity(blocks.len());

    for block in blocks {
        let slots_raw = SignupSlot::list_for_block(state.db(), block.id).await?;
        let mut slots_with_signups = Vec::with_capacity(slots_raw.len());

        for slot in slots_raw {
            let signups = Signup::list_for_slot(state.db(), slot.id).await?;
            let signup_count = signups.len() as i32;
            let is_full = slot.max_count.map(|m| signup_count >= m).unwrap_or(false);
            slots_with_signups.push(SlotWithSignups {
                slot,
                signups,
                is_full,
            });
        }

        result.push(BlockWithSlots {
            block,
            slots: slots_with_signups,
        });
    }

    Ok(Json(result))
}

// ── POST /reunions/:id/schedule ────────────────────────────────────────────────

pub async fn create_block(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<NewScheduleBlock>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    if body.end_time <= body.start_time {
        return Err(AppError::BadRequest(
            "end_time must be after start_time".into(),
        ));
    }

    let block = ScheduleBlock::create(state.db(), reunion_id, user.id, body).await?;
    Ok((StatusCode::CREATED, Json(block)))
}

// ── PATCH /reunions/:id/schedule/:block_id ────────────────────────────────────

pub async fn update_block(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, block_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<NewScheduleBlock>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let block = ScheduleBlock::find_by_id(state.db(), block_id).await?;
    if block.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    if body.end_time <= body.start_time {
        return Err(AppError::BadRequest(
            "end_time must be after start_time".into(),
        ));
    }

    // Re-use NewScheduleBlock as the update shape (all fields replaceable)
    let updated = sqlx::query_as::<_, ScheduleBlock>(
        r#"UPDATE schedule_blocks
           SET block_date = $1, start_time = $2, end_time = $3,
               title = $4, description = $5, block_type = $6,
               location_note = $7, updated_at = NOW()
           WHERE id = $8
           RETURNING *"#,
    )
    .bind(body.block_date)
    .bind(body.start_time)
    .bind(body.end_time)
    .bind(&body.title)
    .bind(&body.description)
    .bind(&body.block_type)
    .bind(&body.location_note)
    .bind(block_id)
    .fetch_optional(state.db())
    .await?
    .ok_or(AppError::NotFound)?;

    Ok(Json(updated))
}

// ── DELETE /reunions/:id/schedule/:block_id ───────────────────────────────────

pub async fn delete_block(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, block_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let block = ScheduleBlock::find_by_id(state.db(), block_id).await?;
    if block.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    ScheduleBlock::delete(state.db(), block_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── POST /reunions/:id/schedule/:block_id/slots ───────────────────────────────

pub async fn create_slot(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, block_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<NewSignupSlot>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let block = ScheduleBlock::find_by_id(state.db(), block_id).await?;
    if block.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    if body.min_count < 0 {
        return Err(AppError::BadRequest("min_count cannot be negative".into()));
    }
    if let Some(max) = body.max_count {
        if max < body.min_count {
            return Err(AppError::BadRequest(
                "max_count cannot be less than min_count".into(),
            ));
        }
    }

    let slot = SignupSlot::create(state.db(), block_id, body).await?;
    Ok((StatusCode::CREATED, Json(slot)))
}

// ── POST /reunions/:id/schedule/:block_id/slots/:slot_id/claim ────────────────

pub async fn claim_slot(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, block_id, slot_id)): Path<(Uuid, Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;

    // Members can sign up during active phase (and schedule phase for prep)
    if !matches!(reunion.phase, Phase::PrepCompleted | Phase::Active) {
        return Err(AppError::WrongPhase {
            required: "schedule or active".into(),
            current: reunion.phase.label().into(),
        });
    }

    // Verify the slot lives under the correct block + reunion
    let block = ScheduleBlock::find_by_id(state.db(), block_id).await?;
    if block.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let signup = Signup::claim(state.db(), slot_id, user.id).await?;
    Ok((StatusCode::CREATED, Json(signup)))
}

// ── POST /reunions/:id/schedule/:block_id/slots/:slot_id/assign ──────────────
// RA override: assign any user to a slot, bypassing phase and capacity checks.

#[derive(Deserialize)]
pub struct AdminAssignRequest {
    pub user_id: uuid::Uuid,
}

pub async fn admin_assign_slot(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, block_id, slot_id)): Path<(uuid::Uuid, uuid::Uuid, uuid::Uuid)>,
    Json(body): Json<AdminAssignRequest>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let block = ScheduleBlock::find_by_id(state.db(), block_id).await?;
    if block.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let signup = Signup::admin_assign(state.db(), slot_id, body.user_id).await?;
    Ok((StatusCode::CREATED, Json(signup)))
}

// ── DELETE /reunions/:id/schedule/:block_id/slots/:slot_id/assign/:user_id ───
// RA override: remove any user's signup (not just self).

pub async fn admin_remove_signup(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, block_id, slot_id, target_user_id)): Path<(
        uuid::Uuid,
        uuid::Uuid,
        uuid::Uuid,
        uuid::Uuid,
    )>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let block = ScheduleBlock::find_by_id(state.db(), block_id).await?;
    if block.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    Signup::release(state.db(), slot_id, target_user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── DELETE /reunions/:id/schedule/:block_id/slots/:slot_id/claim ──────────────

pub async fn release_slot(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, block_id, slot_id)): Path<(Uuid, Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    let reunion = load_reunion(&state, reunion_id).await?;

    if !matches!(reunion.phase, Phase::PrepCompleted | Phase::Active) {
        return Err(AppError::WrongPhase {
            required: "schedule or active".into(),
            current: reunion.phase.label().into(),
        });
    }

    let block = ScheduleBlock::find_by_id(state.db(), block_id).await?;
    if block.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    Signup::release(state.db(), slot_id, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schedule::BlockType;
    use chrono::NaiveTime;

    #[test]
    fn new_schedule_block_deserializes() {
        let json = r#"{
            "block_date": "2026-07-12",
            "start_time": "18:00:00",
            "end_time": "20:00:00",
            "title": "Group Dinner",
            "description": null,
            "block_type": "group",
            "location_note": "Main dining room"
        }"#;
        let block: NewScheduleBlock = serde_json::from_str(json).unwrap();
        assert_eq!(block.title, "Group Dinner");
        assert_eq!(block.block_type, BlockType::Group);
        assert_eq!(block.start_time, NaiveTime::from_hms_opt(18, 0, 0).unwrap());
    }

    #[test]
    fn admin_assign_request_deserializes() {
        let id = uuid::Uuid::new_v4();
        let json = format!(r#"{{"user_id":"{id}"}}"#);
        let req: AdminAssignRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req.user_id, id);
    }

    #[test]
    fn time_ordering_validation() {
        let start = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(20, 0, 0).unwrap();
        assert!(end > start);
        assert!(!(start > end));
    }
}
