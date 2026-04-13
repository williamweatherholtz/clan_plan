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
use tokio::fs;
use uuid::Uuid;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

use crate::{
    auth::session::CurrentUser,
    error::{AppError, AppResult},
    models::media::{extension_for_mime, is_allowed_mime, Media, NewMedia},
    state::AppState,
};

use super::helpers::{load_reunion, user_is_ra};

// ── POST /reunions/:id/media ──────────────────────────────────────────────────

pub async fn upload_media(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    mut multipart: Multipart,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let mut original_filename: Option<String> = None;
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut mime: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}").into()))?
    {
        if field.name() == Some("file") {
            original_filename = field.file_name().map(String::from);
            mime = field.content_type().map(String::from);
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("read error: {e}").into()))?;
            if bytes.len() as u64 > state.config().max_upload_bytes {
                return Err(AppError::BadRequest("file exceeds maximum upload size".into()));
            }
            file_bytes = Some(bytes.to_vec());
            break;
        }
    }

    let bytes =
        file_bytes.ok_or_else(|| AppError::BadRequest("no 'file' field in upload".into()))?;
    let original_filename = original_filename.unwrap_or_else(|| "upload".into());
    let mime = mime.unwrap_or_else(|| "application/octet-stream".into());

    if !is_allowed_mime(&mime) {
        return Err(AppError::BadRequest(
            format!("unsupported file type: {mime}").into(),
        ));
    }

    let ext = extension_for_mime(&mime).unwrap_or("bin");
    let stored_name = format!("{}.{}", Uuid::new_v4(), ext);

    let storage_root = PathBuf::from(&state.config().media_storage_path);
    let reunion_dir = storage_root.join(reunion_id.to_string());
    fs::create_dir_all(&reunion_dir)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("create media dir: {e}")))?;

    let abs_path = reunion_dir.join(&stored_name);
    let relative_path = format!("{}/{}", reunion_id, stored_name);

    fs::write(&abs_path, &bytes)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("write file: {e}")))?;

    let new = NewMedia {
        reunion_id,
        uploaded_by: user.id,
        stored_filename: stored_name,
        original_filename,
        mime_type: mime,
        file_size_bytes: bytes.len() as i64,
        file_path: relative_path,
    };

    let media = Media::create(state.db(), new).await.map_err(|e| {
        // Best-effort cleanup if the DB insert fails
        let _ = std::fs::remove_file(&abs_path);
        e
    })?;

    Ok((StatusCode::CREATED, Json(media)))
}

// ── GET /reunions/:id/media ───────────────────────────────────────────────────

pub async fn list_media(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    let items = Media::list_for_reunion(state.db(), reunion_id).await?;
    Ok(Json(items))
}

// ── GET /reunions/:id/media/:media_id ────────────────────────────────────────

pub async fn download_media(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, media_id)): Path<(Uuid, Uuid)>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    let media = Media::find_by_id(state.db(), media_id).await?;
    if media.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let abs_path = PathBuf::from(&state.config().media_storage_path).join(&media.file_path);
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
    load_reunion(&state, reunion_id).await?;
    let media = Media::find_by_id(state.db(), media_id).await?;
    if media.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    let is_uploader = media.uploaded_by == user.id;
    let is_admin = user_is_ra(&state, &user, reunion_id).await;

    if !is_uploader && !is_admin {
        return Err(AppError::Forbidden);
    }

    let abs_path = PathBuf::from(&state.config().media_storage_path).join(&media.file_path);
    Media::delete(state.db(), media_id).await?;

    // Remove from disk (best-effort — don't fail the request if file is missing)
    if let Err(e) = fs::remove_file(&abs_path).await {
        tracing::warn!("could not delete media file {}: {e}", abs_path.display());
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── GET /reunions/:id/media/download-all ─────────────────────────────────────

pub async fn download_all_zip(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    let items = Media::list_for_reunion(state.db(), reunion_id).await?;

    let storage_root = PathBuf::from(&state.config().media_storage_path);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let buf = {
        let mut inner: Vec<u8> = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(&mut inner));

        for item in &items {
            let abs_path = storage_root.join(&item.file_path);
            match fs::read(&abs_path).await {
                Ok(bytes) => {
                    if zip.start_file(&item.original_filename, options).is_ok() {
                        if let Err(e) = zip.write_all(&bytes) {
                            tracing::warn!("zip write error for media {}: {e}", item.id);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("skipping missing file {}: {e}", abs_path.display());
                }
            }
        }

        zip.finish()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("zip finish: {e}")))?;
        inner
    };

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
