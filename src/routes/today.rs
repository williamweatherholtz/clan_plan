use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
};
use chrono::Local;
use serde::Serialize;
use std::{convert::Infallible, time::Duration};
use tokio_stream::{wrappers::IntervalStream, StreamExt as _};
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
    error::AppResult,
    models::schedule::{ScheduleBlock, Signup, SignupSlot},
    state::AppState,
};

use super::{
    helpers::load_reunion,
    schedule::{BlockWithSlots, SlotWithSignups},
};

// ── GET /reunions/:id/today (SSE) ─────────────────────────────────────────────
//
// Streams today's schedule as a JSON-encoded SSE event, refreshing every 30 s.
// The first tick fires immediately so the client gets data right away.

#[derive(Serialize)]
struct TodaySnapshot {
    date: String, // YYYY-MM-DD
    blocks: Vec<BlockWithSlots>,
}

pub async fn get_today(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    // Validate the reunion exists before opening the stream
    load_reunion(&state, reunion_id).await?;

    let interval = tokio::time::interval(Duration::from_secs(30));
    let stream = IntervalStream::new(interval).then(move |_| {
        let state = state.clone();
        async move {
            let data = match build_snapshot(&state, reunion_id).await {
                Ok(snap) => serde_json::to_string(&snap).unwrap_or_default(),
                Err(e) => {
                    tracing::error!("today-view SSE error for reunion {reunion_id}: {e:?}");
                    r#"{"error":"internal error"}"#.to_owned()
                }
            };
            Ok::<Event, Infallible>(Event::default().data(data))
        }
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

// ── GET /reunions/:id/schedule.ics ───────────────────────────────────────────

pub async fn get_ics(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion(&state, reunion_id).await?;
    let blocks = ScheduleBlock::list_for_reunion(state.db(), reunion_id).await?;

    let mut ics = String::from(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//familyer//EN\r\nCALSCALE:GREGORIAN\r\n",
    );

    for block in &blocks {
        let dtstart = format_ics_datetime(block.block_date, block.start_time);
        let dtend = format_ics_datetime(block.block_date, block.end_time);
        let summary = escape_ics(&block.title);
        let desc = block
            .description
            .as_deref()
            .map(escape_ics)
            .unwrap_or_default();

        ics.push_str(&format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}@familyer\r\n\
             DTSTART:{dtstart}\r\n\
             DTEND:{dtend}\r\n\
             SUMMARY:{summary}\r\n\
             DESCRIPTION:{desc}\r\n\
             END:VEVENT\r\n",
            id = block.id,
        ));
    }

    ics.push_str("END:VCALENDAR\r\n");

    let filename = format!(
        "{}.ics",
        reunion.title.replace(|c: char| !c.is_alphanumeric(), "_")
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Ok(val) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, val);
    }

    Ok((StatusCode::OK, headers, ics))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn build_snapshot(state: &AppState, reunion_id: Uuid) -> crate::error::AppResult<TodaySnapshot> {
    let today = Local::now().date_naive();
    let all_blocks = ScheduleBlock::list_for_reunion(state.db(), reunion_id).await?;
    let mut result = Vec::new();

    for block in all_blocks.into_iter().filter(|b| b.block_date == today) {
        let slots_raw = SignupSlot::list_for_block(state.db(), block.id).await?;
        let mut slots_with_signups = Vec::new();

        for slot in slots_raw {
            let signups = Signup::list_for_slot(state.db(), slot.id).await?;
            let signup_count = signups.len() as i32;
            let is_full = slot.max_count.map(|m| signup_count >= m).unwrap_or(false);
            slots_with_signups.push(SlotWithSignups {
                slot,
                signups,
                is_full,
            });
        }

        result.push(BlockWithSlots {
            block,
            slots: slots_with_signups,
        });
    }

    Ok(TodaySnapshot {
        date: today.to_string(),
        blocks: result,
    })
}

fn format_ics_datetime(date: chrono::NaiveDate, time: chrono::NaiveTime) -> String {
    format!("{}T{}", date.format("%Y%m%d"), time.format("%H%M%S"))
}

fn escape_ics(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveTime};

    #[test]
    fn ics_datetime_format() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        let time = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert_eq!(format_ics_datetime(date, time), "20260712T180000");
    }

    #[test]
    fn ics_datetime_midnight() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        let time = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
        assert_eq!(format_ics_datetime(date, time), "20260712T000000");
    }

    #[test]
    fn ics_escape_special_chars() {
        assert_eq!(escape_ics("a,b;c\nd\\e"), "a\\,b\\;c\\nd\\\\e");
    }

    #[test]
    fn ics_escape_plain_text() {
        assert_eq!(escape_ics("Hello World"), "Hello World");
    }
}
