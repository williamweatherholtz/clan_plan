use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::phase::Phase;

// ── Structs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Reunion {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub phase: Phase,
    pub responsible_admin_id: Option<Uuid>,
    pub selected_location_id: Option<Uuid>,
    pub location_votes_revealed: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReunionDate {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub set_by: Uuid,
    pub set_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct NewReunion {
    pub title: String,
    pub description: Option<String>,
    pub responsible_admin_id: Option<Uuid>,
}

// ── Reunion DB queries ─────────────────────────────────────────────────────────

impl Reunion {
    pub async fn create(pool: &PgPool, new: NewReunion, created_by: Uuid) -> AppResult<Reunion> {
        Ok(sqlx::query_as::<_, Reunion>(
            r#"INSERT INTO reunions (title, description, responsible_admin_id, created_by)
               VALUES ($1, $2, $3, $4)
               RETURNING *"#,
        )
        .bind(&new.title)
        .bind(&new.description)
        .bind(new.responsible_admin_id)
        .bind(created_by)
        .fetch_one(pool)
        .await?)
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>("SELECT * FROM reunions WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn list_all(pool: &PgPool) -> AppResult<Vec<Reunion>> {
        Ok(
            sqlx::query_as::<_, Reunion>("SELECT * FROM reunions ORDER BY created_at DESC")
                .fetch_all(pool)
                .await?,
        )
    }

    /// Advance the phase by one step. Uses an optimistic UPDATE so concurrent
    /// advances produce a clear conflict error rather than silently double-advancing.
    pub async fn advance_phase(pool: &PgPool, reunion_id: Uuid, current: &Phase) -> AppResult<Reunion> {
        let next = current.advance()?;
        sqlx::query_as::<_, Reunion>(
            r#"UPDATE reunions
               SET phase = $1, updated_at = NOW()
               WHERE id = $2 AND phase = $3
               RETURNING *"#,
        )
        .bind(&next)
        .bind(reunion_id)
        .bind(current)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| {
            AppError::Conflict("phase has already changed or reunion not found".into())
        })
    }

    pub async fn update_title_description(
        pool: &PgPool,
        reunion_id: Uuid,
        title: &str,
        description: Option<&str>,
    ) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>(
            "UPDATE reunions SET title = $1, description = $2, updated_at = NOW()
             WHERE id = $3 RETURNING *",
        )
        .bind(title)
        .bind(description)
        .bind(reunion_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    pub async fn set_responsible_admin(
        pool: &PgPool,
        reunion_id: Uuid,
        user_id: Option<Uuid>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE reunions SET responsible_admin_id = $1, updated_at = NOW() WHERE id = $2",
        )
        .bind(user_id)
        .bind(reunion_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn reveal_location_votes(pool: &PgPool, reunion_id: Uuid) -> AppResult<()> {
        sqlx::query(
            "UPDATE reunions SET location_votes_revealed = TRUE, updated_at = NOW() WHERE id = $1",
        )
        .bind(reunion_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Sysadmin emergency override: set phase to any value without precondition checks.
    pub async fn force_set_phase(pool: &PgPool, reunion_id: Uuid, phase: &Phase) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>(
            "UPDATE reunions SET phase = $1, updated_at = NOW() WHERE id = $2 RETURNING *",
        )
        .bind(phase)
        .bind(reunion_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    pub async fn set_selected_location(
        pool: &PgPool,
        reunion_id: Uuid,
        location_id: Uuid,
    ) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>(
            "UPDATE reunions SET selected_location_id = $1, updated_at = NOW()
             WHERE id = $2 RETURNING *",
        )
        .bind(location_id)
        .bind(reunion_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }
}

// ── ReunionDate DB queries ─────────────────────────────────────────────────────

impl ReunionDate {
    /// Replace the current date range for a reunion (only one range is kept).
    pub async fn set(
        pool: &PgPool,
        reunion_id: Uuid,
        start_date: NaiveDate,
        end_date: NaiveDate,
        set_by: Uuid,
    ) -> AppResult<ReunionDate> {
        let mut tx = pool.begin().await?;

        // Clear any previous date selection
        sqlx::query("DELETE FROM reunion_dates WHERE reunion_id = $1")
            .bind(reunion_id)
            .execute(&mut *tx)
            .await?;

        let row = sqlx::query_as::<_, ReunionDate>(
            r#"INSERT INTO reunion_dates (reunion_id, start_date, end_date, set_by)
               VALUES ($1, $2, $3, $4)
               RETURNING *"#,
        )
        .bind(reunion_id)
        .bind(start_date)
        .bind(end_date)
        .bind(set_by)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(row)
    }

    pub async fn find_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Option<ReunionDate>> {
        Ok(sqlx::query_as::<_, ReunionDate>(
            "SELECT * FROM reunion_dates WHERE reunion_id = $1 LIMIT 1",
        )
        .bind(reunion_id)
        .fetch_optional(pool)
        .await?)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase::Phase;

    fn make_reunion(phase: Phase) -> Reunion {
        Reunion {
            id: Uuid::new_v4(),
            title: "Summer 2026".into(),
            description: None,
            phase,
            responsible_admin_id: None,
            selected_location_id: None,
            location_votes_revealed: false,
            created_by: Uuid::new_v4(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn reunion_serializes_phase() {
        let r = make_reunion(Phase::Availability);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"availability\""));
    }

    #[test]
    fn date_range_ordering() {
        let start = NaiveDate::from_ymd_opt(2026, 7, 10).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap();
        assert!(end >= start);
    }
}
