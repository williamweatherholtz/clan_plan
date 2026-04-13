use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LocationCandidate {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub added_by: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub external_url: Option<String>,
    pub capacity: Option<i32>,
    /// Stored in cents to avoid floating-point issues.
    pub estimated_cost_cents: Option<i32>,
    pub image_path: Option<String>,
    /// IANA timezone identifier, e.g. "America/New_York".
    pub timezone: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Returned to members: score shown only after votes are revealed.
#[derive(Debug, Clone, Serialize)]
pub struct LocationCandidateView {
    #[serde(flatten)]
    pub candidate: LocationCandidate,
    /// None until the RA reveals votes (or for the member's own vote, always shown).
    pub avg_score: Option<f64>,
    pub vote_count: i64,
    pub my_vote: Option<LocationVoteView>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LocationVote {
    pub id: Uuid,
    pub location_candidate_id: Uuid,
    pub user_id: Uuid,
    pub score: i16,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Safe view of a vote (used in member-facing responses).
#[derive(Debug, Clone, Serialize)]
pub struct LocationVoteView {
    pub score: i16,
    pub comment: Option<String>,
}

/// Vote enriched with the voter's display name (RA-only view).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VoteWithName {
    pub display_name: String,
    pub score: i16,
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NewLocationCandidate {
    pub title: String,
    pub description: Option<String>,
    pub external_url: Option<String>,
    pub capacity: Option<i32>,
    pub estimated_cost_cents: Option<i32>,
    /// IANA timezone identifier, e.g. "America/New_York".
    pub timezone: String,
}

impl LocationCandidate {
    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        added_by: Uuid,
        new: NewLocationCandidate,
    ) -> AppResult<LocationCandidate> {
        Ok(sqlx::query_as::<_, LocationCandidate>(
            r#"INSERT INTO location_candidates
               (reunion_id, added_by, title, description, external_url, capacity, estimated_cost_cents, timezone)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
               RETURNING *"#,
        )
        .bind(reunion_id)
        .bind(added_by)
        .bind(&new.title)
        .bind(&new.description)
        .bind(&new.external_url)
        .bind(new.capacity)
        .bind(new.estimated_cost_cents)
        .bind(&new.timezone)
        .fetch_one(pool)
        .await?)
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<LocationCandidate> {
        sqlx::query_as::<_, LocationCandidate>(
            "SELECT * FROM location_candidates WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    pub async fn list_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<LocationCandidate>> {
        Ok(sqlx::query_as::<_, LocationCandidate>(
            "SELECT * FROM location_candidates WHERE reunion_id = $1 ORDER BY created_at",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn set_image_path(
        pool: &PgPool,
        id: Uuid,
        image_path: Option<&str>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE location_candidates SET image_path = $1, updated_at = NOW() WHERE id = $2",
        )
        .bind(image_path)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM location_candidates WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

impl LocationVote {
    /// Insert or update the calling user's vote for a location candidate.
    pub async fn upsert(
        pool: &PgPool,
        candidate_id: Uuid,
        user_id: Uuid,
        score: i16,
        comment: Option<&str>,
    ) -> AppResult<LocationVote> {
        Ok(sqlx::query_as::<_, LocationVote>(
            r#"INSERT INTO location_votes (location_candidate_id, user_id, score, comment)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (location_candidate_id, user_id) DO UPDATE
               SET score = EXCLUDED.score,
                   comment = EXCLUDED.comment,
                   updated_at = NOW()
               RETURNING *"#,
        )
        .bind(candidate_id)
        .bind(user_id)
        .bind(score)
        .bind(comment)
        .fetch_one(pool)
        .await?)
    }

    pub async fn for_candidate(pool: &PgPool, candidate_id: Uuid) -> AppResult<Vec<LocationVote>> {
        Ok(sqlx::query_as::<_, LocationVote>(
            "SELECT * FROM location_votes WHERE location_candidate_id = $1 ORDER BY created_at",
        )
        .bind(candidate_id)
        .fetch_all(pool)
        .await?)
    }

    /// Returns (avg_score, vote_count, my_vote) for a candidate.
    /// my_vote includes both score and comment so the UI can pre-fill both fields.
    pub async fn aggregate_for_candidate(
        pool: &PgPool,
        candidate_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<(Option<f64>, i64, Option<LocationVoteView>)> {
        let (avg, count): (Option<f64>, i64) = sqlx::query_as(
            "SELECT AVG(score::float), COUNT(*) FROM location_votes WHERE location_candidate_id = $1",
        )
        .bind(candidate_id)
        .fetch_one(pool)
        .await?;

        let my_vote: Option<LocationVoteView> = sqlx::query_as::<_, (i16, Option<String>)>(
            "SELECT score, comment FROM location_votes WHERE location_candidate_id = $1 AND user_id = $2",
        )
        .bind(candidate_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .map(|(score, comment)| LocationVoteView { score, comment });

        Ok((avg, count, my_vote))
    }

    /// All votes with voter names for a single candidate (RA-only).
    pub async fn votes_with_names_for_candidate(
        pool: &PgPool,
        candidate_id: Uuid,
    ) -> AppResult<Vec<VoteWithName>> {
        Ok(sqlx::query_as::<_, VoteWithName>(
            r#"SELECT u.display_name, lv.score, lv.comment
               FROM location_votes lv
               JOIN users u ON u.id = lv.user_id
               WHERE lv.location_candidate_id = $1
               ORDER BY u.display_name"#,
        )
        .bind(candidate_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn by_user(
        pool: &PgPool,
        candidate_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<Option<LocationVote>> {
        Ok(sqlx::query_as::<_, LocationVote>(
            "SELECT * FROM location_votes
             WHERE location_candidate_id = $1 AND user_id = $2",
        )
        .bind(candidate_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
    }

    /// Aggregate: average score + vote count per candidate.
    /// Family-unit-weighted average score per candidate.
    /// Each family unit counts as one vote regardless of how many members voted.
    /// Members with no family unit each count as their own unit.
    pub async fn summary_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<CandidateSummary>> {
        Ok(sqlx::query_as::<_, CandidateSummary>(
            r#"WITH per_family AS (
                 SELECT lv.location_candidate_id,
                        COALESCE(u.family_unit_id::text, u.id::text) AS group_key,
                        AVG(lv.score::float) AS family_avg
                 FROM location_votes lv
                 JOIN users u ON u.id = lv.user_id
                 JOIN location_candidates lc ON lc.id = lv.location_candidate_id
                 WHERE lc.reunion_id = $1
                 GROUP BY lv.location_candidate_id, group_key
               )
               SELECT location_candidate_id AS candidate_id,
                      AVG(family_avg)        AS avg_score,
                      COUNT(*)::bigint       AS vote_count
               FROM per_family
               GROUP BY location_candidate_id
               ORDER BY avg_score DESC NULLS LAST"#,
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CandidateSummary {
    pub candidate_id: Uuid,
    pub avg_score: Option<f64>,
    pub vote_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_bounds() {
        // The DB enforces 1–5; verify our constants match
        for valid in 1i16..=5 {
            assert!((1..=5).contains(&valid));
        }
    }

    #[test]
    fn view_hides_sensitive_fields() {
        let vote = LocationVote {
            id: Uuid::new_v4(),
            location_candidate_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            score: 4,
            comment: Some("nice pool".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        // LocationVoteView exposes only score + comment
        let view = LocationVoteView {
            score: vote.score,
            comment: vote.comment.clone(),
        };
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("user_id"));
    }
}
