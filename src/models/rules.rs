//! Comment thread attached to the per-reunion rules pane.
//!
//! One thread per reunion (the rules pane is 1:1 with reunion). Shape
//! mirrors `activity_comments` so the existing comment-row partial and
//! posting flow transfer cleanly — only the parent FK differs.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RulesComment {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub user_id: Uuid,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Display-side view: a comment joined with the author's display name so
/// the template doesn't have to do a per-row lookup.
#[derive(Debug, Clone, Serialize)]
pub struct RulesCommentView {
    pub id: Uuid,
    pub user_id: Uuid,
    pub author_name: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

impl RulesComment {
    pub async fn list_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<RulesCommentView>> {
        let rows: Vec<(Uuid, Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT c.id, c.user_id, u.display_name, c.content, c.created_at
             FROM rules_comments c
             JOIN users u ON u.id = c.user_id
             WHERE c.reunion_id = $1
             ORDER BY c.created_at",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, user_id, author_name, content, created_at)| RulesCommentView {
                id,
                user_id,
                author_name,
                content,
                created_at,
            })
            .collect())
    }

    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        user_id: Uuid,
        content: &str,
    ) -> AppResult<RulesComment> {
        Ok(sqlx::query_as::<_, RulesComment>(
            "INSERT INTO rules_comments (reunion_id, user_id, content)
             VALUES ($1, $2, $3)
             RETURNING *",
        )
        .bind(reunion_id)
        .bind(user_id)
        .bind(content)
        .fetch_one(pool)
        .await?)
    }

    /// Delete a comment. Only the author or an admin should call this; the
    /// check lives in the route handler since this layer doesn't know what
    /// "admin" means in the current request.
    pub async fn delete(pool: &PgPool, comment_id: Uuid) -> AppResult<()> {
        let res = sqlx::query("DELETE FROM rules_comments WHERE id = $1")
            .bind(comment_id)
            .execute(pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<RulesComment> {
        sqlx::query_as::<_, RulesComment>("SELECT * FROM rules_comments WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }
}
