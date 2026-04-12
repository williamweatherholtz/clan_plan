use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

// ── Enums ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "activity_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Proposed,
    /// RA has featured this idea to draw attention to it.
    Pinned,
    /// RA has promoted this idea to a schedule block.
    Scheduled,
    /// Will not happen; kept for transparency.
    Cancelled,
}

impl ActivityStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ActivityStatus::Proposed => "proposed",
            ActivityStatus::Pinned => "pinned",
            ActivityStatus::Scheduled => "scheduled",
            ActivityStatus::Cancelled => "cancelled",
        }
    }
}

// ── Structs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ActivityIdea {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub proposed_by: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub needs_time_slot: bool,
    pub suggested_time: Option<String>,
    pub status: ActivityStatus,
    pub promoted_to_block_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ActivityVote {
    pub id: Uuid,
    pub activity_idea_id: Uuid,
    pub user_id: Uuid,
    pub interest_score: i16,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ActivityComment {
    pub id: Uuid,
    pub activity_idea_id: Uuid,
    pub user_id: Uuid,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Aggregate interest summary for displaying in list views.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ActivitySummary {
    pub idea_id: Uuid,
    pub avg_interest: Option<f64>,
    pub vote_count: i64,
    pub comment_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct NewActivityIdea {
    pub title: String,
    pub description: Option<String>,
    pub needs_time_slot: bool,
    pub suggested_time: Option<String>,
}

// ── ActivityIdea queries ───────────────────────────────────────────────────────

impl ActivityIdea {
    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        proposed_by: Uuid,
        new: NewActivityIdea,
    ) -> AppResult<ActivityIdea> {
        Ok(sqlx::query_as::<_, ActivityIdea>(
            r#"INSERT INTO activity_ideas
               (reunion_id, proposed_by, title, description, needs_time_slot, suggested_time)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING *"#,
        )
        .bind(reunion_id)
        .bind(proposed_by)
        .bind(&new.title)
        .bind(&new.description)
        .bind(new.needs_time_slot)
        .bind(&new.suggested_time)
        .fetch_one(pool)
        .await?)
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<ActivityIdea> {
        sqlx::query_as::<_, ActivityIdea>("SELECT * FROM activity_ideas WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn list_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<ActivityIdea>> {
        Ok(sqlx::query_as::<_, ActivityIdea>(
            "SELECT * FROM activity_ideas WHERE reunion_id = $1 ORDER BY status, created_at DESC",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn set_status(
        pool: &PgPool,
        idea_id: Uuid,
        status: &ActivityStatus,
    ) -> AppResult<ActivityIdea> {
        sqlx::query_as::<_, ActivityIdea>(
            "UPDATE activity_ideas SET status = $1, updated_at = NOW() WHERE id = $2 RETURNING *",
        )
        .bind(status)
        .bind(idea_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    pub async fn promote_to_block(
        pool: &PgPool,
        idea_id: Uuid,
        block_id: Uuid,
    ) -> AppResult<ActivityIdea> {
        sqlx::query_as::<_, ActivityIdea>(
            r#"UPDATE activity_ideas
               SET status = 'scheduled', promoted_to_block_id = $1, updated_at = NOW()
               WHERE id = $2
               RETURNING *"#,
        )
        .bind(block_id)
        .bind(idea_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    /// Aggregated interest score + comment count for a single idea.
    pub async fn for_idea(pool: &PgPool, idea_id: Uuid) -> AppResult<ActivitySummary> {
        Ok(sqlx::query_as::<_, ActivitySummary>(
            r#"SELECT
                ai.id AS idea_id,
                AVG(av.interest_score::float) AS avg_interest,
                COUNT(DISTINCT av.id)          AS vote_count,
                COUNT(DISTINCT ac.id)          AS comment_count
               FROM activity_ideas ai
               LEFT JOIN activity_votes    av ON av.activity_idea_id = ai.id
               LEFT JOIN activity_comments ac ON ac.activity_idea_id = ai.id
               WHERE ai.id = $1
               GROUP BY ai.id"#,
        )
        .bind(idea_id)
        .fetch_one(pool)
        .await?)
    }

    /// Aggregated interest scores + comment counts for an entire reunion.
    pub async fn summaries_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<ActivitySummary>> {
        Ok(sqlx::query_as::<_, ActivitySummary>(
            r#"SELECT
                ai.id AS idea_id,
                AVG(av.interest_score::float) AS avg_interest,
                COUNT(DISTINCT av.id)          AS vote_count,
                COUNT(DISTINCT ac.id)          AS comment_count
               FROM activity_ideas ai
               LEFT JOIN activity_votes    av ON av.activity_idea_id = ai.id
               LEFT JOIN activity_comments ac ON ac.activity_idea_id = ai.id
               WHERE ai.reunion_id = $1
               GROUP BY ai.id
               ORDER BY avg_interest DESC NULLS LAST"#,
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }
}

// ── ActivityVote queries ───────────────────────────────────────────────────────

impl ActivityVote {
    pub async fn upsert(
        pool: &PgPool,
        idea_id: Uuid,
        user_id: Uuid,
        interest_score: i16,
    ) -> AppResult<ActivityVote> {
        Ok(sqlx::query_as::<_, ActivityVote>(
            r#"INSERT INTO activity_votes (activity_idea_id, user_id, interest_score)
               VALUES ($1, $2, $3)
               ON CONFLICT (activity_idea_id, user_id) DO UPDATE
               SET interest_score = EXCLUDED.interest_score, updated_at = NOW()
               RETURNING *"#,
        )
        .bind(idea_id)
        .bind(user_id)
        .bind(interest_score)
        .fetch_one(pool)
        .await?)
    }

    pub async fn by_user(
        pool: &PgPool,
        idea_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<Option<ActivityVote>> {
        Ok(sqlx::query_as::<_, ActivityVote>(
            "SELECT * FROM activity_votes WHERE activity_idea_id = $1 AND user_id = $2",
        )
        .bind(idea_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
    }
}

// ── ActivityComment queries ────────────────────────────────────────────────────

impl ActivityComment {
    pub async fn create(
        pool: &PgPool,
        idea_id: Uuid,
        user_id: Uuid,
        content: &str,
    ) -> AppResult<ActivityComment> {
        Ok(sqlx::query_as::<_, ActivityComment>(
            r#"INSERT INTO activity_comments (activity_idea_id, user_id, content)
               VALUES ($1, $2, $3)
               RETURNING *"#,
        )
        .bind(idea_id)
        .bind(user_id)
        .bind(content)
        .fetch_one(pool)
        .await?)
    }

    pub async fn list_for_idea(
        pool: &PgPool,
        idea_id: Uuid,
    ) -> AppResult<Vec<ActivityComment>> {
        Ok(sqlx::query_as::<_, ActivityComment>(
            "SELECT * FROM activity_comments WHERE activity_idea_id = $1 ORDER BY created_at",
        )
        .bind(idea_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn delete(pool: &PgPool, comment_id: Uuid, requesting_user_id: Uuid, is_admin: bool) -> AppResult<()> {
        let query = if is_admin {
            "DELETE FROM activity_comments WHERE id = $1"
        } else {
            "DELETE FROM activity_comments WHERE id = $1 AND user_id = $2"
        };

        let result = if is_admin {
            sqlx::query(query).bind(comment_id).execute(pool).await?
        } else {
            sqlx::query(query).bind(comment_id).bind(requesting_user_id).execute(pool).await?
        };

        if result.rows_affected() == 0 {
            return Err(AppError::Forbidden);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_transitions() {
        // Proposed is the default; Scheduled means it has a block
        assert_ne!(ActivityStatus::Proposed, ActivityStatus::Scheduled);
        assert_ne!(ActivityStatus::Pinned, ActivityStatus::Cancelled);
    }

    #[test]
    fn interest_score_range() {
        for valid in 1i16..=5 {
            assert!((1..=5).contains(&valid));
        }
    }
}
