use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReunionInvite {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub token: String,
    pub created_by: Uuid,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

/// Member who joined via invite — enriched with user details for the RA view.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct InviteMember {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    pub joined_at: DateTime<Utc>,
}

impl ReunionInvite {
    /// Generate a new invite token for a reunion (RA action).
    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        created_by: Uuid,
    ) -> AppResult<ReunionInvite> {
        // 256-bit token: two UUID v4s concatenated without hyphens.
        let token = format!(
            "{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        Ok(sqlx::query_as::<_, ReunionInvite>(
            "INSERT INTO reunion_invites (reunion_id, token, created_by)
             VALUES ($1, $2, $3) RETURNING *",
        )
        .bind(reunion_id)
        .bind(&token)
        .bind(created_by)
        .fetch_one(pool)
        .await?)
    }

    /// Look up an active invite by its token.
    pub async fn find_by_token(pool: &PgPool, token: &str) -> AppResult<ReunionInvite> {
        sqlx::query_as::<_, ReunionInvite>(
            "SELECT * FROM reunion_invites WHERE token = $1 AND active = TRUE",
        )
        .bind(token)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
    }

    /// List all active invites for a reunion.
    pub async fn list_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<ReunionInvite>> {
        Ok(sqlx::query_as::<_, ReunionInvite>(
            "SELECT * FROM reunion_invites
             WHERE reunion_id = $1 AND active = TRUE
             ORDER BY created_at DESC",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    /// Revoke (deactivate) an invite so it can no longer be used.
    pub async fn deactivate(pool: &PgPool, id: Uuid, reunion_id: Uuid) -> AppResult<()> {
        let rows = sqlx::query(
            "UPDATE reunion_invites SET active = FALSE
             WHERE id = $1 AND reunion_id = $2",
        )
        .bind(id)
        .bind(reunion_id)
        .execute(pool)
        .await?
        .rows_affected();
        if rows == 0 { Err(AppError::NotFound) } else { Ok(()) }
    }

    /// Add a user to a reunion as a direct (unassigned) member via this invite.
    /// Idempotent — safe to call if the user already redeemed.
    pub async fn redeem(
        pool: &PgPool,
        invite: &ReunionInvite,
        user_id: Uuid,
    ) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO reunion_invite_members (reunion_id, user_id, invite_id)
             VALUES ($1, $2, $3)
             ON CONFLICT (reunion_id, user_id) DO NOTHING",
        )
        .bind(invite.reunion_id)
        .bind(user_id)
        .bind(invite.id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// List all members who joined a reunion via invite link.
    pub async fn list_members(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<InviteMember>> {
        Ok(sqlx::query_as::<_, InviteMember>(
            r#"SELECT u.id AS user_id, u.display_name, u.email, rim.joined_at
               FROM reunion_invite_members rim
               JOIN users u ON u.id = rim.user_id
               WHERE rim.reunion_id = $1
               ORDER BY rim.joined_at"#,
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    /// Remove a user's direct invite-based membership from a reunion.
    pub async fn remove_member(
        pool: &PgPool,
        reunion_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<()> {
        sqlx::query(
            "DELETE FROM reunion_invite_members WHERE reunion_id = $1 AND user_id = $2",
        )
        .bind(reunion_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Check whether a user is a direct (invite-based) member of a reunion.
    pub async fn is_direct_member(
        pool: &PgPool,
        reunion_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
               SELECT 1 FROM reunion_invite_members
               WHERE reunion_id = $1 AND user_id = $2
             )",
        )
        .bind(reunion_id)
        .bind(user_id)
        .fetch_one(pool)
        .await?)
    }
}
