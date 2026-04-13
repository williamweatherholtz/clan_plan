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
    pub slug: Option<String>,
    pub phase: Phase,
    pub selected_location_id: Option<Uuid>,
    pub location_votes_revealed: bool,
    /// RA-set date range for the availability poll calendar.
    /// When None, falls back to confirmed reunion dates or a 90-day window.
    pub avail_poll_start: Option<NaiveDate>,
    pub avail_poll_end: Option<NaiveDate>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Assumed duration (minutes) for activities with no explicit end time.
    /// Configurable by the RA; defaults to 60.
    pub default_activity_duration_minutes: i32,
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

/// Join record tracking which family units participate in a reunion.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReunionFamilyUnit {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub family_unit_id: Uuid,
    pub added_at: DateTime<Utc>,
}

impl ReunionFamilyUnit {
    /// Returns the IDs of family units enrolled in this reunion.
    pub async fn list_ids_for_reunion(pool: &PgPool, reunion_id: Uuid) -> AppResult<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT family_unit_id FROM reunion_family_units WHERE reunion_id = $1 ORDER BY added_at",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    pub async fn add(pool: &PgPool, reunion_id: Uuid, family_unit_id: Uuid) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO reunion_family_units (reunion_id, family_unit_id)
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(reunion_id)
        .bind(family_unit_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn remove(pool: &PgPool, reunion_id: Uuid, family_unit_id: Uuid) -> AppResult<()> {
        sqlx::query(
            "DELETE FROM reunion_family_units WHERE reunion_id = $1 AND family_unit_id = $2",
        )
        .bind(reunion_id)
        .bind(family_unit_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct NewReunion {
    pub title: String,
    pub description: Option<String>,
}

// ── Reunion DB queries ─────────────────────────────────────────────────────────

impl Reunion {
    pub async fn create(pool: &PgPool, new: NewReunion, created_by: Uuid) -> AppResult<Reunion> {
        Ok(sqlx::query_as::<_, Reunion>(
            r#"INSERT INTO reunions (title, description, created_by)
               VALUES ($1, $2, $3)
               RETURNING *"#,
        )
        .bind(&new.title)
        .bind(&new.description)
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

    /// Force-set the phase to any value. Used for RA phase retreat.
    pub async fn set_phase(pool: &PgPool, reunion_id: Uuid, phase: &Phase) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>(
            "UPDATE reunions SET phase = $1, updated_at = NOW() WHERE id = $2 RETURNING *",
        )
        .bind(phase)
        .bind(reunion_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
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

    pub async fn find_by_slug(pool: &PgPool, slug: &str) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>("SELECT * FROM reunions WHERE slug = $1")
            .bind(slug)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn set_slug(pool: &PgPool, reunion_id: Uuid, slug: Option<&str>) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>(
            "UPDATE reunions SET slug = $1, updated_at = NOW() WHERE id = $2 RETURNING *",
        )
        .bind(slug)
        .bind(reunion_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    /// List only non-archived reunions, most recent first.
    pub async fn list_active(pool: &PgPool) -> AppResult<Vec<Reunion>> {
        Ok(sqlx::query_as::<_, Reunion>(
            "SELECT * FROM reunions WHERE phase != 'archived' ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await?)
    }

    /// Set (or clear) the RA's explicit availability poll window.
    pub async fn set_avail_poll_window(
        pool: &PgPool,
        reunion_id: Uuid,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
    ) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>(
            "UPDATE reunions SET avail_poll_start = $1, avail_poll_end = $2, updated_at = NOW()
             WHERE id = $3 RETURNING *",
        )
        .bind(start)
        .bind(end)
        .bind(reunion_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    pub async fn set_default_activity_duration(
        pool: &PgPool,
        reunion_id: Uuid,
        minutes: i32,
    ) -> AppResult<Reunion> {
        sqlx::query_as::<_, Reunion>(
            "UPDATE reunions SET default_activity_duration_minutes = $1, updated_at = NOW()
             WHERE id = $2 RETURNING *",
        )
        .bind(minutes)
        .bind(reunion_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    /// Permanently delete a reunion and all related data (cascaded by FK constraints).
    pub async fn delete(pool: &PgPool, reunion_id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM reunions WHERE id = $1")
            .bind(reunion_id)
            .execute(pool)
            .await?;
        Ok(())
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

// ── ReunionAdmin ──────────────────────────────────────────────────────────────

/// Join record: a user who has RA access to a reunion.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReunionAdmin {
    pub reunion_id: Uuid,
    pub user_id: Uuid,
    pub added_at: DateTime<Utc>,
}

impl ReunionAdmin {
    pub async fn list_ids_for_reunion(pool: &PgPool, reunion_id: Uuid) -> AppResult<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM reunion_admins WHERE reunion_id = $1 ORDER BY added_at",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    pub async fn add(pool: &PgPool, reunion_id: Uuid, user_id: Uuid) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO reunion_admins (reunion_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(reunion_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn remove(pool: &PgPool, reunion_id: Uuid, user_id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM reunion_admins WHERE reunion_id = $1 AND user_id = $2")
            .bind(reunion_id)
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
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
            slug: None,
            phase,
            selected_location_id: None,
            location_votes_revealed: false,
            avail_poll_start: None,
            avail_poll_end: None,
            created_by: Uuid::new_v4(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            default_activity_duration_minutes: 60,
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
