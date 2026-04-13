use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppResult;

// ── Announcements ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Announcement {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub posted_by: Uuid,
    pub title: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct NewAnnouncement {
    pub title: String,
    pub content: String,
}

impl Announcement {
    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        posted_by: Uuid,
        new: NewAnnouncement,
    ) -> AppResult<Announcement> {
        Ok(sqlx::query_as::<_, Announcement>(
            r#"INSERT INTO announcements (reunion_id, posted_by, title, content)
               VALUES ($1, $2, $3, $4)
               RETURNING *"#,
        )
        .bind(reunion_id)
        .bind(posted_by)
        .bind(&new.title)
        .bind(&new.content)
        .fetch_one(pool)
        .await?)
    }

    pub async fn list_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<Announcement>> {
        Ok(sqlx::query_as::<_, Announcement>(
            "SELECT * FROM announcements WHERE reunion_id = $1 ORDER BY created_at DESC",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM announcements WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

