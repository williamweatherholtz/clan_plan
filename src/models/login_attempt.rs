use sqlx::PgPool;

use crate::error::{AppError, AppResult};

pub struct LoginAttempt;

/// Window within which we count failures.
const WINDOW_MINUTES: i64 = 15;
/// Max failures before lockout.
pub const MAX_FAILURES: i64 = 10;

impl LoginAttempt {
    /// Record a failed login attempt.
    pub async fn record(pool: &PgPool, email: &str, ip: &str) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO login_attempts (email, ip) VALUES ($1, $2)",
        )
        .bind(email)
        .bind(ip)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
        Ok(())
    }

    /// Count failed attempts for an email within the rolling window.
    pub async fn recent_count(pool: &PgPool, email: &str) -> AppResult<i64> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM login_attempts
             WHERE email = $1
               AND attempted_at > NOW() - ($2 || ' minutes')::INTERVAL",
        )
        .bind(email)
        .bind(WINDOW_MINUTES)
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)?;
        Ok(count)
    }
}
