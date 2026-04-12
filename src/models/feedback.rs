use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppResult;

// ── Live feedback (available during Active phase onward) ───────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Feedback {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub user_id: Uuid,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Feedback {
    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        user_id: Uuid,
        content: &str,
    ) -> AppResult<Feedback> {
        Ok(sqlx::query_as::<_, Feedback>(
            r#"INSERT INTO feedback (reunion_id, user_id, content)
               VALUES ($1, $2, $3)
               RETURNING *"#,
        )
        .bind(reunion_id)
        .bind(user_id)
        .bind(content)
        .fetch_one(pool)
        .await?)
    }

    pub async fn list_for_reunion(pool: &PgPool, reunion_id: Uuid) -> AppResult<Vec<Feedback>> {
        Ok(sqlx::query_as::<_, Feedback>(
            "SELECT * FROM feedback WHERE reunion_id = $1 ORDER BY created_at",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }
}

// ── Post-reunion survey ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SurveyQuestion {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub question_text: String,
    pub order_index: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SurveyResponse {
    pub id: Uuid,
    pub survey_question_id: Uuid,
    pub user_id: Uuid,
    pub response_text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct NewSurveyQuestion {
    pub question_text: String,
    pub order_index: i32,
}

impl SurveyQuestion {
    /// Seed the default post-reunion questions for a reunion.
    pub async fn seed_defaults(pool: &PgPool, reunion_id: Uuid) -> AppResult<()> {
        let defaults = [
            (0, "What went well this reunion?"),
            (1, "What would you change for next time?"),
            (2, "Do you have any interest in hosting the next reunion?"),
            (3, "Any other thoughts or suggestions?"),
        ];

        for (order_index, question_text) in defaults {
            sqlx::query(
                r#"INSERT INTO survey_questions (reunion_id, question_text, order_index)
                   VALUES ($1, $2, $3)
                   ON CONFLICT DO NOTHING"#,
            )
            .bind(reunion_id)
            .bind(question_text)
            .bind(order_index)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        new: NewSurveyQuestion,
    ) -> AppResult<SurveyQuestion> {
        Ok(sqlx::query_as::<_, SurveyQuestion>(
            r#"INSERT INTO survey_questions (reunion_id, question_text, order_index)
               VALUES ($1, $2, $3)
               RETURNING *"#,
        )
        .bind(reunion_id)
        .bind(&new.question_text)
        .bind(new.order_index)
        .fetch_one(pool)
        .await?)
    }

    pub async fn list_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<SurveyQuestion>> {
        Ok(sqlx::query_as::<_, SurveyQuestion>(
            "SELECT * FROM survey_questions WHERE reunion_id = $1 ORDER BY order_index",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn delete(pool: &PgPool, question_id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM survey_questions WHERE id = $1")
            .bind(question_id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

impl SurveyResponse {
    pub async fn upsert(
        pool: &PgPool,
        question_id: Uuid,
        user_id: Uuid,
        response_text: &str,
    ) -> AppResult<SurveyResponse> {
        Ok(sqlx::query_as::<_, SurveyResponse>(
            r#"INSERT INTO survey_responses (survey_question_id, user_id, response_text)
               VALUES ($1, $2, $3)
               ON CONFLICT (survey_question_id, user_id) DO UPDATE
               SET response_text = EXCLUDED.response_text
               RETURNING *"#,
        )
        .bind(question_id)
        .bind(user_id)
        .bind(response_text)
        .fetch_one(pool)
        .await?)
    }

    /// All responses for a reunion — visible to RA/sysadmin only.
    pub async fn list_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<SurveyResponse>> {
        Ok(sqlx::query_as::<_, SurveyResponse>(
            r#"SELECT sr.* FROM survey_responses sr
               JOIN survey_questions sq ON sq.id = sr.survey_question_id
               WHERE sq.reunion_id = $1
               ORDER BY sq.order_index, sr.created_at"#,
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }
}
