use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

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

// ── Notifications ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub user_id: Uuid,
    pub content: String,
    pub link: Option<String>,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Notification {
    pub async fn create(
        pool: &PgPool,
        user_id: Uuid,
        content: &str,
        link: Option<&str>,
    ) -> AppResult<Notification> {
        Ok(sqlx::query_as::<_, Notification>(
            r#"INSERT INTO notifications (user_id, content, link)
               VALUES ($1, $2, $3)
               RETURNING *"#,
        )
        .bind(user_id)
        .bind(content)
        .bind(link)
        .fetch_one(pool)
        .await?)
    }

    /// Unread notifications for a user, newest first.
    pub async fn list_unread(pool: &PgPool, user_id: Uuid) -> AppResult<Vec<Notification>> {
        Ok(sqlx::query_as::<_, Notification>(
            "SELECT * FROM notifications WHERE user_id = $1 AND read_at IS NULL ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn unread_count(pool: &PgPool, user_id: Uuid) -> AppResult<i64> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM notifications WHERE user_id = $1 AND read_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(pool)
        .await?;
        Ok(row.0)
    }

    pub async fn mark_read(pool: &PgPool, id: Uuid, user_id: Uuid) -> AppResult<()> {
        let n = sqlx::query(
            "UPDATE notifications SET read_at = NOW()
             WHERE id = $1 AND user_id = $2 AND read_at IS NULL",
        )
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    pub async fn mark_all_read(pool: &PgPool, user_id: Uuid) -> AppResult<u64> {
        let n = sqlx::query(
            "UPDATE notifications SET read_at = NOW() WHERE user_id = $1 AND read_at IS NULL",
        )
        .bind(user_id)
        .execute(pool)
        .await?
        .rows_affected();
        Ok(n)
    }
}
