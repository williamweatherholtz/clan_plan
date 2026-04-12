use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

// ── Enums ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "block_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BlockType {
    /// Everyone is expected to attend if at all possible.
    Group,
    /// Come if you want; low pressure.
    Optional,
    /// A meal event (inherits group/optional from context; RA decides).
    Meal,
    /// Arrival or departure block.
    Travel,
}

impl BlockType {
    pub fn color(&self) -> &'static str {
        match self {
            BlockType::Group => "#3B82F6",    // blue
            BlockType::Optional => "#22C55E", // green
            BlockType::Meal => "#F59E0B",     // amber
            BlockType::Travel => "#8B5CF6",   // violet
        }
    }

    pub fn is_attendance_expected(&self) -> bool {
        *self == BlockType::Group || *self == BlockType::Meal
    }

    pub fn label(&self) -> &'static str {
        match self {
            BlockType::Group => "group",
            BlockType::Optional => "optional",
            BlockType::Meal => "meal",
            BlockType::Travel => "travel",
        }
    }
}

// ── Structs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScheduleBlock {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub block_date: NaiveDate,
    pub start_time: NaiveTime,
    pub end_time: NaiveTime,
    pub title: String,
    pub description: Option<String>,
    pub block_type: BlockType,
    pub location_note: Option<String>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SignupSlot {
    pub id: Uuid,
    pub schedule_block_id: Uuid,
    pub role_name: String,
    pub description: Option<String>,
    pub min_count: i32,
    pub max_count: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Signup {
    pub id: Uuid,
    pub signup_slot_id: Uuid,
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct NewScheduleBlock {
    pub block_date: NaiveDate,
    pub start_time: NaiveTime,
    pub end_time: NaiveTime,
    pub title: String,
    pub description: Option<String>,
    pub block_type: BlockType,
    pub location_note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NewSignupSlot {
    pub role_name: String,
    pub description: Option<String>,
    pub min_count: i32,
    pub max_count: Option<i32>,
}

// ── ScheduleBlock queries ──────────────────────────────────────────────────────

impl ScheduleBlock {
    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        created_by: Uuid,
        new: NewScheduleBlock,
    ) -> AppResult<ScheduleBlock> {
        Ok(sqlx::query_as::<_, ScheduleBlock>(
            r#"INSERT INTO schedule_blocks
               (reunion_id, block_date, start_time, end_time, title, description,
                block_type, location_note, created_by)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
               RETURNING *"#,
        )
        .bind(reunion_id)
        .bind(new.block_date)
        .bind(new.start_time)
        .bind(new.end_time)
        .bind(&new.title)
        .bind(&new.description)
        .bind(&new.block_type)
        .bind(&new.location_note)
        .bind(created_by)
        .fetch_one(pool)
        .await?)
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<ScheduleBlock> {
        sqlx::query_as::<_, ScheduleBlock>("SELECT * FROM schedule_blocks WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn list_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<ScheduleBlock>> {
        Ok(sqlx::query_as::<_, ScheduleBlock>(
            "SELECT * FROM schedule_blocks
             WHERE reunion_id = $1
             ORDER BY block_date, start_time",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    /// All blocks for a specific day — used by the "today" view.
    pub async fn list_for_date(
        pool: &PgPool,
        reunion_id: Uuid,
        date: NaiveDate,
    ) -> AppResult<Vec<ScheduleBlock>> {
        Ok(sqlx::query_as::<_, ScheduleBlock>(
            "SELECT * FROM schedule_blocks
             WHERE reunion_id = $1 AND block_date = $2
             ORDER BY start_time",
        )
        .bind(reunion_id)
        .bind(date)
        .fetch_all(pool)
        .await?)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM schedule_blocks WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

// ── SignupSlot queries ─────────────────────────────────────────────────────────

impl SignupSlot {
    pub async fn create(
        pool: &PgPool,
        schedule_block_id: Uuid,
        new: NewSignupSlot,
    ) -> AppResult<SignupSlot> {
        Ok(sqlx::query_as::<_, SignupSlot>(
            r#"INSERT INTO signup_slots
               (schedule_block_id, role_name, description, min_count, max_count)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING *"#,
        )
        .bind(schedule_block_id)
        .bind(&new.role_name)
        .bind(&new.description)
        .bind(new.min_count)
        .bind(new.max_count)
        .fetch_one(pool)
        .await?)
    }

    pub async fn list_for_block(
        pool: &PgPool,
        schedule_block_id: Uuid,
    ) -> AppResult<Vec<SignupSlot>> {
        Ok(sqlx::query_as::<_, SignupSlot>(
            "SELECT * FROM signup_slots WHERE schedule_block_id = $1 ORDER BY created_at",
        )
        .bind(schedule_block_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn current_count(pool: &PgPool, slot_id: Uuid) -> AppResult<i64> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM signups WHERE signup_slot_id = $1",
        )
        .bind(slot_id)
        .fetch_one(pool)
        .await?;
        Ok(row.0)
    }
}

// ── Signup queries ─────────────────────────────────────────────────────────────

impl Signup {
    /// Claim a slot. Enforces `max_count` if set — returns Conflict if full.
    pub async fn claim(pool: &PgPool, slot_id: Uuid, user_id: Uuid) -> AppResult<Signup> {
        let mut tx = pool.begin().await?;

        let slot = sqlx::query_as::<_, SignupSlot>(
            "SELECT * FROM signup_slots WHERE id = $1 FOR UPDATE",
        )
        .bind(slot_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        if let Some(max) = slot.max_count {
            let count: i64 = sqlx::query_as::<_, (i64,)>(
                "SELECT COUNT(*) FROM signups WHERE signup_slot_id = $1",
            )
            .bind(slot_id)
            .fetch_one(&mut *tx)
            .await?
            .0;

            if count >= max as i64 {
                return Err(AppError::Conflict("signup slot is full".into()));
            }
        }

        let row = sqlx::query_as::<_, Signup>(
            r#"INSERT INTO signups (signup_slot_id, user_id)
               VALUES ($1, $2)
               ON CONFLICT (signup_slot_id, user_id) DO NOTHING
               RETURNING *"#,
        )
        .bind(slot_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::Conflict("already signed up for this slot".into()))?;

        tx.commit().await?;
        Ok(row)
    }

    pub async fn release(pool: &PgPool, slot_id: Uuid, user_id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM signups WHERE signup_slot_id = $1 AND user_id = $2")
            .bind(slot_id)
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// RA admin override: assign any user to a slot, ignoring phase and capacity limits.
    pub async fn admin_assign(pool: &PgPool, slot_id: Uuid, user_id: Uuid) -> AppResult<Signup> {
        sqlx::query_as::<_, Signup>(
            r#"INSERT INTO signups (signup_slot_id, user_id)
               VALUES ($1, $2)
               ON CONFLICT (signup_slot_id, user_id) DO NOTHING
               RETURNING *"#,
        )
        .bind(slot_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::Conflict("user is already in this slot".into()))
    }

    pub async fn list_for_slot(pool: &PgPool, slot_id: Uuid) -> AppResult<Vec<Signup>> {
        Ok(sqlx::query_as::<_, Signup>(
            "SELECT * FROM signups WHERE signup_slot_id = $1 ORDER BY created_at",
        )
        .bind(slot_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn list_for_user_in_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<Vec<Signup>> {
        Ok(sqlx::query_as::<_, Signup>(
            r#"SELECT s.* FROM signups s
               JOIN signup_slots ss ON ss.id = s.signup_slot_id
               JOIN schedule_blocks sb ON sb.id = ss.schedule_block_id
               WHERE sb.reunion_id = $1 AND s.user_id = $2
               ORDER BY s.created_at"#,
        )
        .bind(reunion_id)
        .bind(user_id)
        .fetch_all(pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_block_expects_attendance() {
        assert!(BlockType::Group.is_attendance_expected());
        assert!(BlockType::Meal.is_attendance_expected());
        assert!(!BlockType::Optional.is_attendance_expected());
        assert!(!BlockType::Travel.is_attendance_expected());
    }

    #[test]
    fn all_block_types_have_colors() {
        for bt in [
            BlockType::Group,
            BlockType::Optional,
            BlockType::Meal,
            BlockType::Travel,
        ] {
            assert!(!bt.color().is_empty());
        }
    }
}
