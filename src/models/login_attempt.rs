use sqlx::PgPool;

use crate::error::{AppError, AppResult};

pub struct LoginAttempt;

/// Window within which we count failures.
const WINDOW_MINUTES: i64 = 15;
/// Max failures before lockout.
pub const MAX_FAILURES: i64 = 10;
/// Delete failed-login records older than this many days.
const CLEANUP_DAYS: i64 = 7;

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

    /// Count failed attempts within the rolling window, partitioned by email
    /// and by IP. Lockout fires when **either** count exceeds MAX_FAILURES
    /// — counting only by email is bypassable by rotating addresses, and the
    /// schema's `ip` column + `idx_login_attempts_ip_time` index were
    /// originally designed for this dual check.
    pub async fn recent_count(
        pool: &PgPool,
        email: &str,
        ip: &str,
    ) -> AppResult<(i64, i64)> {
        let row: (i64, i64) = sqlx::query_as(
            "SELECT
                COUNT(*) FILTER (WHERE email = $1) AS email_count,
                COUNT(*) FILTER (WHERE ip = $2)    AS ip_count
             FROM login_attempts
             WHERE attempted_at > NOW() - ($3 || ' minutes')::INTERVAL",
        )
        .bind(email)
        .bind(ip)
        .bind(WINDOW_MINUTES)
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)?;
        Ok(row)
    }

    /// Drop rows older than `CLEANUP_DAYS`. Called once at app startup to
    /// keep the table bounded — historical attempts after a week have no
    /// rate-limiting value and only bloat the `idx_login_attempts_*_time`
    /// indexes. Returns the number of rows deleted (for logging).
    pub async fn cleanup_old(pool: &PgPool) -> AppResult<u64> {
        let result = sqlx::query(
            "DELETE FROM login_attempts
             WHERE attempted_at < NOW() - ($1 || ' days')::INTERVAL",
        )
        .bind(CLEANUP_DAYS)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
        Ok(result.rows_affected())
    }
}
