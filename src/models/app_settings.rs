use sqlx::PgPool;

use crate::error::{AppError, AppResult};

#[derive(sqlx::FromRow)]
pub struct AppSettings {
    pub registration_enabled: bool,
}

impl AppSettings {
    pub async fn get(pool: &PgPool) -> AppResult<Self> {
        sqlx::query_as::<_, Self>(
            "SELECT registration_enabled FROM app_settings WHERE id = 1",
        )
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)
    }

    pub async fn set_registration_enabled(pool: &PgPool, enabled: bool) -> AppResult<()> {
        sqlx::query(
            "UPDATE app_settings
             SET registration_enabled = $1, updated_at = NOW()
             WHERE id = 1",
        )
        .bind(enabled)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
        Ok(())
    }
}
