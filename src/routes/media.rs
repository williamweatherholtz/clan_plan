use axum::{
    extract::{Multipart, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use std::{
    io::{Cursor, Write},
    path::PathBuf,
};
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

use crate::{
    auth::session::CurrentUser,
    error::{AppError, AppResult},
    models::media::{extension_for_mime, is_allowed_mime, Media, NewMedia},
    state::AppState,
};

use super::helpers::{load_reunion, load_reunion_for_api_member, user_is_ra};

// ── POST /reunions/:id/media ──────────────────────────────────────────────────

pub async fn upload_media(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    mut multipart: Multipart,
) -> AppResult<impl IntoResponse> {
    load_reunion_for_api_member(&state, &user, reunion_id).await?;

    // Read the config ceiling once so we don't keep a borrow of AppState
    // alive across the chunk-streaming await points below.
    let max_bytes = state.config().max_upload_bytes;
    let storage_root = PathBuf::from(&state.config().media_storage_path);

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}").into()))?
    {
        if field.name() != Some("file") {
            continue;
        }

        let original_filename = field
            .file_name()
            .map(String::from)
            .unwrap_or_else(|| "upload".into());
        let mime = field
            .content_type()
            .map(String::from)
            .unwrap_or_else(|| "application/octet-stream".into());

        if !is_allowed_mime(&mime) {
            return Err(AppError::BadRequest(
                format!("unsupported file type: {mime}").into(),
            ));
        }

        let ext = extension_for_mime(&mime).unwrap_or("bin");
        let stored_name = format!("{}.{}", Uuid::new_v4(), ext);
        let reunion_dir = storage_root.join(reunion_id.to_string());
        fs::create_dir_all(&reunion_dir)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("create media dir: {e}")))?;
        let abs_path = reunion_dir.join(&stored_name);
        let relative_path = format!("{}/{}", reunion_id, stored_name);

        // Stream chunks straight to disk. For a 5 GiB upload this keeps
        // peak RAM at ~chunk size (~8-64 KiB) instead of the full file.
        // Size is enforced incrementally so an oversize upload aborts as
        // soon as we cross the ceiling rather than after all bytes arrive.
        let mut file = fs::File::create(&abs_path)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("create file: {e}")))?;
        let mut total: u64 = 0;

        loop {
            let next = field
                .chunk()
                .await
                .map_err(|e| AppError::BadRequest(format!("read chunk: {e}").into()));
            match next {
                Ok(Some(chunk)) => {
                    total = total.saturating_add(chunk.len() as u64);
                    if total > max_bytes {
                        drop(file);
                        let _ = fs::remove_file(&abs_path).await;
                        return Err(AppError::BadRequest(
                            "file exceeds maximum upload size".into(),
                        ));
                    }
                    if let Err(e) = file.write_all(&chunk).await {
                        drop(file);
                        let _ = fs::remove_file(&abs_path).await;
                        return Err(AppError::Internal(anyhow::anyhow!("write chunk: {e}")));
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    drop(file);
                    let _ = fs::remove_file(&abs_path).await;
                    return Err(e);
                }
            }
        }

        if let Err(e) = file.flush().await {
            let _ = fs::remove_file(&abs_path).await;
            return Err(AppError::Internal(anyhow::anyhow!("flush file: {e}")));
        }
        drop(file);

        // Empty / aborted upload — don't leave a zero-byte tombstone row.
        if total == 0 {
            let _ = fs::remove_file(&abs_path).await;
            return Err(AppError::BadRequest("empty upload".into()));
        }

        let new = NewMedia {
            reunion_id,
            uploaded_by: user.id,
            stored_filename: stored_name,
            original_filename,
            mime_type: mime,
            file_size_bytes: total as i64,
            file_path: relative_path,
        };

        let media = match Media::create(state.db(), new).await {
            Ok(m) => m,
            Err(e) => {
                let _ = fs::remove_file(&abs_path).await;
                return Err(e);
            }
        };

        return Ok((StatusCode::CREATED, Json(media)));
    }

    Err(AppError::BadRequest("no 'file' field in upload".into()))
}

// ── GET /reunions/:id/media ───────────────────────────────────────────────────

pub async fn list_media(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion_for_api_member(&state, &user, reunion_id).await?;
    let items = Media::list_for_reunion(state.db(), reunion_id).await?;
    Ok(Json(items))
}

// ── GET /reunions/:id/media/:media_id ────────────────────────────────────────

pub async fn download_media(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, media_id)): Path<(Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    load_reunion_for_api_member(&state, &user, reunion_id).await?;
    let media = Media::find_by_id(state.db(), media_id).await?;
    if media.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let abs_path = safe_media_path(&state.config().media_storage_path, &media.file_path)
        .await
        .ok_or(AppError::NotFound)?;
    let bytes = fs::read(&abs_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read file: {e}")))?;

    let mut headers = HeaderMap::new();
    if let Ok(ct) = HeaderValue::from_str(&media.mime_type) {
        headers.insert(header::CONTENT_TYPE, ct);
    }
    let disp = format!("attachment; filename=\"{}\"", media.original_filename);
    if let Ok(cd) = HeaderValue::from_str(&disp) {
        headers.insert(header::CONTENT_DISPOSITION, cd);
    }

    Ok((StatusCode::OK, headers, bytes))
}

// ── DELETE /reunions/:id/media/:media_id ─────────────────────────────────────

pub async fn delete_media(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, media_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion_for_api_member(&state, &user, reunion_id).await?;
    let media = Media::find_by_id(state.db(), media_id).await?;
    if media.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let is_uploader = media.uploaded_by == user.id;
    let is_admin = user_is_ra(&state, &user, reunion_id).await;

    if !is_uploader && !is_admin {
        return Err(AppError::Forbidden);
    }

    let abs_path = safe_media_path(&state.config().media_storage_path, &media.file_path)
        .await
        .ok_or(AppError::NotFound)?;
    Media::delete(state.db(), media_id).await?;

    // Remove from disk (best-effort — don't fail the request if file is missing)
    if let Err(e) = fs::remove_file(&abs_path).await {
        tracing::warn!("could not delete media file {}: {e}", abs_path.display());
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── GET /reunions/:id/media/download-all ─────────────────────────────────────

pub async fn download_all_zip(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion_for_api_member(&state, &user, reunion_id).await?;
    let items = Media::list_for_reunion(state.db(), reunion_id).await?;

    let storage_root_str = state.config().media_storage_path.clone();

    // Resolve every member's absolute path AND its byte length up front so we
    // can (a) refuse archives that would balloon memory and (b) hand the
    // (path, name) list to a blocking worker for the actual zip build.
    // Hard cap of 500 MiB is generous for most family reunions but stops the
    // 100-file × 25 MiB pathological case the critique flagged.
    const ZIP_MAX_BYTES: u64 = 500 * 1024 * 1024;

    let mut entries: Vec<(PathBuf, String, u64)> = Vec::with_capacity(items.len());
    let mut total_bytes: u64 = 0;
    for item in &items {
        let Some(abs_path) = safe_media_path(&storage_root_str, &item.file_path).await else {
            tracing::warn!("skipping media {} — path outside storage root", item.id);
            continue;
        };
        let len = match fs::metadata(&abs_path).await {
            Ok(m) => m.len(),
            Err(e) => {
                tracing::warn!("skipping missing file {}: {e}", abs_path.display());
                continue;
            }
        };
        total_bytes = total_bytes.saturating_add(len);
        if total_bytes > ZIP_MAX_BYTES {
            return Err(AppError::BadRequest(
                "the zip archive would exceed 500 MiB — download individual files instead".into(),
            ));
        }
        entries.push((abs_path, item.original_filename.clone(), len));
    }

    // ZipWriter does compressed, CPU-bound work and reads files synchronously.
    // Running it on the tokio runtime worker would stall every other request
    // for the duration; spawn_blocking parks it on the blocking pool instead.
    let buf = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, std::io::Error> {
        let mut inner: Vec<u8> = Vec::with_capacity(total_bytes as usize / 2);
        let mut zip = ZipWriter::new(Cursor::new(&mut inner));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        for (abs_path, name, _len) in &entries {
            match std::fs::read(abs_path) {
                Ok(bytes) => {
                    if zip.start_file(name, options).is_ok() {
                        if let Err(e) = zip.write_all(&bytes) {
                            tracing::warn!("zip write error for {}: {e}", abs_path.display());
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("skipping disappeared file {}: {e}", abs_path.display());
                }
            }
        }

        zip.finish().map_err(std::io::Error::other)?;
        Ok(inner)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("zip join: {e}")))?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("zip finish: {e}")))?;

    let filename = format!(
        "{}_media.zip",
        reunion.title.replace(|c: char| !c.is_alphanumeric(), "_")
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Ok(val) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, val);
    }

    Ok((StatusCode::OK, headers, buf))
}

// ── S-14: Path canonicalization helper ───────────────────────────────────────
// Resolves `file_path` relative to `storage_root`, then verifies the result
// stays within the root directory to prevent path traversal attacks.
// Returns None if the path escapes the root or canonicalization fails.
async fn safe_media_path(storage_root: &str, file_path: &str) -> Option<PathBuf> {
    let root = fs::canonicalize(PathBuf::from(storage_root)).await.ok()?;
    let candidate = root.join(file_path);
    // canonicalize requires the path to exist
    let canonical = fs::canonicalize(&candidate).await.ok()?;
    if canonical.starts_with(&root) {
        Some(canonical)
    } else {
        tracing::warn!(
            "path traversal attempt: {} escaped storage root",
            file_path
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::models::media::is_allowed_mime;

    #[test]
    fn jpeg_is_allowed() {
        assert!(is_allowed_mime("image/jpeg"));
    }

    #[test]
    fn pdf_is_rejected() {
        assert!(!is_allowed_mime("application/pdf"));
    }
}
