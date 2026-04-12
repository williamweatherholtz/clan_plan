use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct HostRotation {
    pub id: Uuid,
    pub family_unit_id: Uuid,
    pub reunion_id: Option<Uuid>,
    pub is_next: bool,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct NewHostRotation {
    pub family_unit_id: Uuid,
    pub reunion_id: Option<Uuid>,
    pub notes: Option<String>,
}

impl HostRotation {
    pub async fn list_all(pool: &PgPool) -> AppResult<Vec<HostRotation>> {
        Ok(
            sqlx::query_as::<_, HostRotation>(
                "SELECT * FROM host_rotation ORDER BY created_at",
            )
            .fetch_all(pool)
            .await?,
        )
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<HostRotation> {
        sqlx::query_as::<_, HostRotation>("SELECT * FROM host_rotation WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn create(pool: &PgPool, new: NewHostRotation) -> AppResult<HostRotation> {
        Ok(sqlx::query_as::<_, HostRotation>(
            r#"INSERT INTO host_rotation (family_unit_id, reunion_id, notes)
               VALUES ($1, $2, $3)
               RETURNING *"#,
        )
        .bind(new.family_unit_id)
        .bind(new.reunion_id)
        .bind(&new.notes)
        .fetch_one(pool)
        .await?)
    }

    /// Mark `id` as next host. Clears the previous is_next marker first
    /// (the partial unique index permits only one TRUE row at a time).
    pub async fn set_next(pool: &PgPool, id: Uuid) -> AppResult<HostRotation> {
        let mut tx = pool.begin().await?;

        sqlx::query("UPDATE host_rotation SET is_next = FALSE WHERE is_next = TRUE")
            .execute(&mut *tx)
            .await?;

        let entry = sqlx::query_as::<_, HostRotation>(
            "UPDATE host_rotation SET is_next = TRUE WHERE id = $1 RETURNING *",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        tx.commit().await?;
        Ok(entry)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM host_rotation WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_host_rotation_deserializes() {
        let id = Uuid::new_v4();
        let json = format!(r#"{{"family_unit_id":"{id}","reunion_id":null,"notes":"First time hosts"}}"#);
        let new: NewHostRotation = serde_json::from_str(&json).unwrap();
        assert_eq!(new.family_unit_id, id);
        assert!(new.reunion_id.is_none());
        assert_eq!(new.notes.as_deref(), Some("First time hosts"));
    }
}
