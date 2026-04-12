use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppResult;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Availability {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub user_id: Uuid,
    pub available_date: NaiveDate,
    pub created_at: DateTime<Utc>,
}

/// One row per date in the heatmap: how many distinct members are free that day.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct HeatmapEntry {
    pub available_date: NaiveDate,
    pub member_count: i64,
}

impl Availability {
    /// Replace a member's full availability set for a reunion atomically.
    /// Pass an empty slice to clear all availability.
    pub async fn replace(
        pool: &PgPool,
        reunion_id: Uuid,
        user_id: Uuid,
        dates: &[NaiveDate],
    ) -> AppResult<Vec<Availability>> {
        let mut tx = pool.begin().await?;

        sqlx::query("DELETE FROM availability WHERE reunion_id = $1 AND user_id = $2")
            .bind(reunion_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

        let mut rows = Vec::with_capacity(dates.len());
        for date in dates {
            let row = sqlx::query_as::<_, Availability>(
                r#"INSERT INTO availability (reunion_id, user_id, available_date)
                   VALUES ($1, $2, $3)
                   RETURNING *"#,
            )
            .bind(reunion_id)
            .bind(user_id)
            .bind(date)
            .fetch_one(&mut *tx)
            .await?;
            rows.push(row);
        }

        tx.commit().await?;
        Ok(rows)
    }

    /// All dates a specific member has marked available for a reunion.
    pub async fn for_user(
        pool: &PgPool,
        reunion_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<Vec<NaiveDate>> {
        let rows = sqlx::query_as::<_, (NaiveDate,)>(
            "SELECT available_date FROM availability
             WHERE reunion_id = $1 AND user_id = $2
             ORDER BY available_date",
        )
        .bind(reunion_id)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(|(d,)| d).collect())
    }

    /// Aggregate heatmap: each date → number of members free that day.
    /// Used by the RA to pick reunion dates.
    pub async fn heatmap(pool: &PgPool, reunion_id: Uuid) -> AppResult<Vec<HeatmapEntry>> {
        Ok(sqlx::query_as::<_, HeatmapEntry>(
            r#"SELECT available_date, COUNT(DISTINCT user_id) AS member_count
               FROM availability
               WHERE reunion_id = $1
               GROUP BY available_date
               ORDER BY available_date"#,
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    /// Total number of members who have submitted any availability.
    pub async fn respondent_count(pool: &PgPool, reunion_id: Uuid) -> AppResult<i64> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(DISTINCT user_id) FROM availability WHERE reunion_id = $1",
        )
        .bind(reunion_id)
        .fetch_one(pool)
        .await?;
        Ok(row.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heatmap_entry_serializes() {
        let entry = HeatmapEntry {
            available_date: NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            member_count: 7,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("7"));
    }
}
