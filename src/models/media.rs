use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// Allowed upload MIME types. Checked before writing to disk.
pub const ALLOWED_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "video/mp4",
    "video/quicktime", // .mov
];

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Media {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub uploaded_by: Uuid,
    /// UUID-based name on disk — never user-controlled (prevents path traversal).
    pub stored_filename: String,
    pub original_filename: String,
    pub mime_type: String,
    pub file_size_bytes: i64,
    /// Relative path from MEDIA_STORAGE_PATH root.
    pub file_path: String,
    pub created_at: DateTime<Utc>,
}

pub struct NewMedia {
    pub reunion_id: Uuid,
    pub uploaded_by: Uuid,
    pub stored_filename: String,
    pub original_filename: String,
    pub mime_type: String,
    pub file_size_bytes: i64,
    pub file_path: String,
}

impl Media {
    pub async fn create(pool: &PgPool, new: NewMedia) -> AppResult<Media> {
        Ok(sqlx::query_as::<_, Media>(
            r#"INSERT INTO media
               (reunion_id, uploaded_by, stored_filename, original_filename,
                mime_type, file_size_bytes, file_path)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING *"#,
        )
        .bind(new.reunion_id)
        .bind(new.uploaded_by)
        .bind(&new.stored_filename)
        .bind(&new.original_filename)
        .bind(&new.mime_type)
        .bind(new.file_size_bytes)
        .bind(&new.file_path)
        .fetch_one(pool)
        .await?)
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Media> {
        sqlx::query_as::<_, Media>("SELECT * FROM media WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn list_for_reunion(pool: &PgPool, reunion_id: Uuid) -> AppResult<Vec<Media>> {
        Ok(sqlx::query_as::<_, Media>(
            "SELECT * FROM media WHERE reunion_id = $1 ORDER BY created_at DESC",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    /// Delete DB record. Caller is responsible for removing the file on disk.
    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<Media> {
        sqlx::query_as::<_, Media>("DELETE FROM media WHERE id = $1 RETURNING *")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    /// Total bytes used by a reunion's media — for storage stats.
    pub async fn total_bytes_for_reunion(pool: &PgPool, reunion_id: Uuid) -> AppResult<i64> {
        let row = sqlx::query_as::<_, (Option<i64>,)>(
            "SELECT SUM(file_size_bytes) FROM media WHERE reunion_id = $1",
        )
        .bind(reunion_id)
        .fetch_one(pool)
        .await?;
        Ok(row.0.unwrap_or(0))
    }
}

/// Returns the extension for a known MIME type, or None for unknown types.
pub fn extension_for_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "video/mp4" => Some("mp4"),
        "video/quicktime" => Some("mov"),
        _ => None,
    }
}

pub fn is_allowed_mime(mime: &str) -> bool {
    ALLOWED_MIME_TYPES.contains(&mime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_mimes_have_extensions() {
        for mime in ALLOWED_MIME_TYPES {
            assert!(
                extension_for_mime(mime).is_some(),
                "no extension for allowed MIME: {mime}"
            );
        }
    }

    #[test]
    fn unknown_mime_rejected() {
        assert!(!is_allowed_mime("application/pdf"));
        assert!(!is_allowed_mime("text/html"));
    }

    #[test]
    fn allowed_mimes_pass() {
        assert!(is_allowed_mime("image/jpeg"));
        assert!(is_allowed_mime("video/mp4"));
    }
}
