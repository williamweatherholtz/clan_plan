use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

// ── Enums ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "user_role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Sysadmin,
    Member,
}

impl UserRole {
    pub fn label(&self) -> &'static str {
        match self {
            UserRole::Sysadmin => "sysadmin",
            UserRole::Member => "member",
        }
    }
}

// ── Structs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    /// Never serialized to API responses.
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    #[serde(skip_serializing)]
    pub google_id: Option<String>,
    pub family_unit_id: Option<Uuid>,
    pub role: UserRole,
    pub avatar_url: Option<String>,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deactivated_at: Option<DateTime<Utc>>,
}

impl User {
    pub fn is_sysadmin(&self) -> bool {
        self.role == UserRole::Sysadmin
    }

    pub fn is_active(&self) -> bool {
        self.deactivated_at.is_none()
    }

    pub fn is_email_verified(&self) -> bool {
        self.email_verified_at.is_some()
    }

    /// True when the user can act as RA for the given reunion.
    pub fn is_ra_for(&self, reunion_responsible_admin_id: Option<Uuid>) -> bool {
        self.is_sysadmin() || reunion_responsible_admin_id == Some(self.id)
    }
}

#[derive(Debug)]
pub struct NewUser {
    pub email: String,
    pub display_name: String,
    pub password_hash: Option<String>,
    pub google_id: Option<String>,
    pub family_unit_id: Option<Uuid>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FamilyUnit {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

// ── Token tables (used by auth module) ────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub struct EmailVerification {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct PasswordReset {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// ── User DB queries ────────────────────────────────────────────────────────────

impl User {
    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<User> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn find_by_email(pool: &PgPool, email: &str) -> AppResult<Option<User>> {
        Ok(
            sqlx::query_as::<_, User>("SELECT * FROM users WHERE LOWER(email) = LOWER($1)")
                .bind(email)
                .fetch_optional(pool)
                .await?,
        )
    }

    pub async fn find_by_google_id(pool: &PgPool, google_id: &str) -> AppResult<Option<User>> {
        Ok(
            sqlx::query_as::<_, User>("SELECT * FROM users WHERE google_id = $1")
                .bind(google_id)
                .fetch_optional(pool)
                .await?,
        )
    }

    pub async fn create(pool: &PgPool, new_user: NewUser) -> AppResult<User> {
        sqlx::query_as::<_, User>(
            r#"INSERT INTO users
               (email, display_name, password_hash, google_id, family_unit_id, avatar_url)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING *"#,
        )
        .bind(&new_user.email)
        .bind(&new_user.display_name)
        .bind(&new_user.password_hash)
        .bind(&new_user.google_id)
        .bind(new_user.family_unit_id)
        .bind(&new_user.avatar_url)
        .fetch_one(pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.constraint() == Some("users_email_key") => {
                AppError::Conflict("email address is already registered".into())
            }
            _ => AppError::Database(e),
        })
    }

    pub async fn update_display_name(
        pool: &PgPool,
        user_id: Uuid,
        display_name: &str,
    ) -> AppResult<User> {
        sqlx::query_as::<_, User>(
            "UPDATE users SET display_name = $1, updated_at = NOW() WHERE id = $2 RETURNING *",
        )
        .bind(display_name)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    pub async fn update_password_hash(pool: &PgPool, user_id: Uuid, hash: &str) -> AppResult<()> {
        sqlx::query("UPDATE users SET password_hash = $1, updated_at = NOW() WHERE id = $2")
            .bind(hash)
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn set_avatar(pool: &PgPool, user_id: Uuid, url: Option<&str>) -> AppResult<()> {
        sqlx::query("UPDATE users SET avatar_url = $1, updated_at = NOW() WHERE id = $2")
            .bind(url)
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn mark_email_verified(pool: &PgPool, user_id: Uuid) -> AppResult<()> {
        sqlx::query(
            "UPDATE users SET email_verified_at = NOW(), updated_at = NOW()
             WHERE id = $1 AND email_verified_at IS NULL",
        )
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_deactivated(pool: &PgPool, user_id: Uuid, deactivate: bool) -> AppResult<()> {
        sqlx::query(
            "UPDATE users SET deactivated_at = CASE WHEN $1 THEN NOW() ELSE NULL END,
             updated_at = NOW() WHERE id = $2",
        )
        .bind(deactivate)
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_role(pool: &PgPool, user_id: Uuid, role: &UserRole) -> AppResult<()> {
        sqlx::query("UPDATE users SET role = $1, updated_at = NOW() WHERE id = $2")
            .bind(role)
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn set_family_unit(
        pool: &PgPool,
        user_id: Uuid,
        family_unit_id: Option<Uuid>,
    ) -> AppResult<()> {
        sqlx::query("UPDATE users SET family_unit_id = $1, updated_at = NOW() WHERE id = $2")
            .bind(family_unit_id)
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn attach_google_id(pool: &PgPool, user_id: Uuid, google_id: &str) -> AppResult<()> {
        let n = sqlx::query(
            "UPDATE users SET google_id = $1, updated_at = NOW() WHERE id = $2",
        )
        .bind(google_id)
        .bind(user_id)
        .execute(pool)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    pub async fn list_all(pool: &PgPool) -> AppResult<Vec<User>> {
        Ok(
            sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at")
                .fetch_all(pool)
                .await?,
        )
    }

    /// Active, email-verified users — the target audience for broadcast notifications.
    pub async fn list_active_verified(pool: &PgPool) -> AppResult<Vec<User>> {
        Ok(sqlx::query_as::<_, User>(
            "SELECT * FROM users
             WHERE deactivated_at IS NULL AND email_verified_at IS NOT NULL
             ORDER BY created_at",
        )
        .fetch_all(pool)
        .await?)
    }
}

// ── FamilyUnit DB queries ──────────────────────────────────────────────────────

impl FamilyUnit {
    pub async fn create(pool: &PgPool, name: &str) -> AppResult<FamilyUnit> {
        Ok(
            sqlx::query_as::<_, FamilyUnit>(
                "INSERT INTO family_units (name) VALUES ($1) RETURNING *",
            )
            .bind(name)
            .fetch_one(pool)
            .await?,
        )
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<FamilyUnit> {
        sqlx::query_as::<_, FamilyUnit>("SELECT * FROM family_units WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn list_all(pool: &PgPool) -> AppResult<Vec<FamilyUnit>> {
        Ok(
            sqlx::query_as::<_, FamilyUnit>("SELECT * FROM family_units ORDER BY name")
                .fetch_all(pool)
                .await?,
        )
    }

    pub async fn rename(pool: &PgPool, id: Uuid, name: &str) -> AppResult<FamilyUnit> {
        sqlx::query_as::<_, FamilyUnit>(
            "UPDATE family_units SET name = $1 WHERE id = $2 RETURNING *",
        )
        .bind(name)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }
}

// ── EmailVerification / PasswordReset token queries ────────────────────────────

impl EmailVerification {
    pub async fn create(pool: &PgPool, user_id: Uuid, token: &str) -> AppResult<()> {
        sqlx::query(
            r#"INSERT INTO email_verifications (user_id, token, expires_at)
               VALUES ($1, $2, NOW() + INTERVAL '24 hours')"#,
        )
        .bind(user_id)
        .bind(token)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Finds a valid (unused, unexpired) token.
    pub async fn consume(pool: &PgPool, token: &str) -> AppResult<EmailVerification> {
        let row = sqlx::query_as::<_, EmailVerification>(
            r#"UPDATE email_verifications
               SET used_at = NOW()
               WHERE token = $1
                 AND used_at IS NULL
                 AND expires_at > NOW()
               RETURNING *"#,
        )
        .bind(token)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::BadRequest("verification token is invalid or expired".into()))?;
        Ok(row)
    }
}

impl PasswordReset {
    pub async fn create(pool: &PgPool, user_id: Uuid, token: &str) -> AppResult<()> {
        // Invalidate any existing unused tokens for this user first
        sqlx::query(
            "UPDATE password_resets SET used_at = NOW()
             WHERE user_id = $1 AND used_at IS NULL",
        )
        .bind(user_id)
        .execute(pool)
        .await?;

        sqlx::query(
            r#"INSERT INTO password_resets (user_id, token, expires_at)
               VALUES ($1, $2, NOW() + INTERVAL '1 hour')"#,
        )
        .bind(user_id)
        .bind(token)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn consume(pool: &PgPool, token: &str) -> AppResult<PasswordReset> {
        sqlx::query_as::<_, PasswordReset>(
            r#"UPDATE password_resets
               SET used_at = NOW()
               WHERE token = $1
                 AND used_at IS NULL
                 AND expires_at > NOW()
               RETURNING *"#,
        )
        .bind(token)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::BadRequest("reset token is invalid or expired".into()))
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_user(role: UserRole, deactivated: bool) -> User {
        User {
            id: Uuid::new_v4(),
            email: "test@example.com".into(),
            display_name: "Test User".into(),
            password_hash: Some("hash".into()),
            google_id: None,
            family_unit_id: None,
            role,
            avatar_url: None,
            email_verified_at: Some(Utc::now()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deactivated_at: if deactivated { Some(Utc::now()) } else { None },
        }
    }

    #[test]
    fn sysadmin_role_check() {
        assert!(make_user(UserRole::Sysadmin, false).is_sysadmin());
        assert!(!make_user(UserRole::Member, false).is_sysadmin());
    }

    #[test]
    fn active_check() {
        assert!(make_user(UserRole::Member, false).is_active());
        assert!(!make_user(UserRole::Member, true).is_active());
    }

    #[test]
    fn ra_check() {
        let user = make_user(UserRole::Member, false);
        let reunion_admin_id = Some(user.id);
        let other_id = Some(Uuid::new_v4());

        assert!(user.is_ra_for(reunion_admin_id));
        assert!(!user.is_ra_for(other_id));
        assert!(!user.is_ra_for(None));

        // Sysadmin is always RA
        let sysadmin = make_user(UserRole::Sysadmin, false);
        assert!(sysadmin.is_ra_for(None));
        assert!(sysadmin.is_ra_for(other_id));
    }
}
