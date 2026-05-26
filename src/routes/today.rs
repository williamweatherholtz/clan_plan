use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
};
use chrono::Utc;
use chrono_tz::Tz;
use serde::Serialize;
use std::{convert::Infallible, time::Duration};
use tokio_stream::{wrappers::IntervalStream, StreamExt as _};
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
    error::AppResult,
    models::{
        location::LocationCandidate,
        schedule::{CancelledScheduleBlock, ScheduleBlock, Signup, SignupSlot},
    },
    state::AppState,
};

use super::{
    helpers::{get_reunion_tz_string, load_reunion, load_reunion_for_api_member, meal_rsvp_names_for_block},
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
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion_for_api_member(&state, &user, reunion_id).await?;
    let tz_str = get_reunion_tz_string(&state, &reunion).await;
    let tz: Tz = tz_str.parse().unwrap_or(chrono_tz::UTC);

    let interval = tokio::time::interval(Duration::from_secs(30));
    let stream = IntervalStream::new(interval).then(move |_| {
        let state = state.clone();
        async move {
            let data = match build_snapshot(&state, reunion_id, tz).await {
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
//
// RFC 5545 compliance notes:
//  • DTSTAMP is required on every VEVENT (§3.6.1).
//  • TZID in DTSTART/DTEND requires a matching VTIMEZONE block in the same
//    calendar object (§3.2.19). Rather than embed a full VTIMEZONE definition,
//    we convert all times to UTC — signalled by the trailing "Z" — which is
//    universally accepted and needs no VTIMEZONE component.

pub async fn get_ics(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let reunion = load_reunion_for_api_member(&state, &user, reunion_id).await?;
    let blocks = ScheduleBlock::list_for_reunion(state.db(), reunion_id).await?;
    let cancelled = CancelledScheduleBlock::list_for_reunion(state.db(), reunion_id).await?;
    let tz_str = get_reunion_tz_string(&state, &reunion).await;
    let tz: Tz = tz_str.parse().unwrap_or(chrono_tz::UTC);

    // Selected reunion location supplies a map-parseable address fallback.
    // Format: "<title>, <address>" so the title is visible AND the address is
    // detected by iOS Calendar (Apple Maps) and Google Calendar.
    let reunion_location_str: Option<String> = if let Some(loc_id) = reunion.selected_location_id {
        match LocationCandidate::find_by_id(state.db(), loc_id).await {
            Ok(loc) => match (loc.address.as_deref(), loc.title.as_str()) {
                (Some(addr), title) if !addr.is_empty() => Some(format!("{title}, {addr}")),
                (_, title) => Some(title.to_string()),
            },
            Err(_) => None,
        }
    } else {
        None
    };

    // DTSTAMP = when this calendar was generated (required by RFC 5545 §3.6.1).
    let dtstamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    let mut ics = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//clanplan//EN\r\n\
         CALSCALE:GREGORIAN\r\n\
         X-WR-TIMEZONE:{tz_str}\r\n",
    );

    for block in &blocks {
        // Convert stored local times to UTC so no VTIMEZONE block is needed.
        let dtstart = local_to_utc(block.block_date, block.start_time, tz);
        let dtend   = local_to_utc(block.block_date, block.end_time,   tz);
        let summary = escape_ics(&block.title);
        // LOCATION assembly: prefer block.location_note when set, AND always
        // append the reunion's selected location (with address) so map apps
        // can hand off to navigation. If the block has no note we just use
        // the reunion location alone.
        let block_loc = block.location_note.as_deref().unwrap_or("").trim();
        let reunion_loc = reunion_location_str.as_deref().unwrap_or("").trim();
        let combined: String = match (block_loc.is_empty(), reunion_loc.is_empty()) {
            (true,  true)  => String::new(),
            (false, true)  => block_loc.to_string(),
            (true,  false) => reunion_loc.to_string(),
            (false, false) => format!("{block_loc} — {reunion_loc}"),
        };
        let location = if combined.is_empty() { String::new() } else { escape_ics(&combined) };

        // DESCRIPTION = block.description + meal make/cleanup commitments, joined
        // by the iCal-escaped line break "\n" (RFC 5545 §3.3.11). Each piece is
        // individually escape_ics()'d; the literal "Make:"/"Cleanup:" labels are
        // safe ASCII and need no escaping.
        let mut desc_parts: Vec<String> = Vec::new();
        if let Some(d) = &block.description {
            desc_parts.push(escape_ics(d));
        }
        let (make_names, cleanup_names) =
            meal_rsvp_names_for_block(state.db(), block.id).await;
        if !make_names.is_empty() {
            desc_parts.push(format!("Make: {}", escape_ics(&make_names.join(", "))));
        }
        if !cleanup_names.is_empty() {
            desc_parts.push(format!("Cleanup: {}", escape_ics(&cleanup_names.join(", "))));
        }
        let desc = desc_parts.join("\\n");

        // SEQUENCE (§3.8.7.4) + LAST-MODIFIED (§3.8.7.3) are what tell a
        // subscribed calendar "this is a newer revision of an event you've
        // already imported by UID, apply the changes." Without them many
        // clients silently ignore re-imported events. We use updated_at's
        // unix-epoch seconds for SEQUENCE — it's monotonically non-decreasing
        // per block (the DB sets it on every UPDATE), so the value only ever
        // climbs across edits.
        let sequence  = block.updated_at.timestamp();
        let last_mod  = block.updated_at.format("%Y%m%dT%H%M%SZ");

        ics.push_str(&format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}@clanplan\r\n\
             DTSTAMP:{dtstamp}\r\n\
             DTSTART:{dtstart}\r\n\
             DTEND:{dtend}\r\n\
             SUMMARY:{summary}\r\n\
             SEQUENCE:{sequence}\r\n\
             LAST-MODIFIED:{last_mod}\r\n",
            id = block.id,
        ));
        if !desc.is_empty() {
            ics.push_str(&format!("DESCRIPTION:{desc}\r\n"));
        }
        if !location.is_empty() {
            ics.push_str(&format!("LOCATION:{location}\r\n"));
        }
        ics.push_str("END:VEVENT\r\n");
    }

    // Tombstones — emit a STATUS:CANCELLED VEVENT for every block that has
    // been deleted since the calendar was first imported. Calendars match by
    // UID and remove the event when STATUS:CANCELLED + a higher SEQUENCE
    // arrives, so subscribers don't keep stale ghost events.
    //
    // RFC 5545 §3.6.1: even cancellations need DTSTART/DTEND/SUMMARY so a
    // client can render and identify the event before honoring the status.
    // We snapshotted those at deletion time in cancelled_schedule_blocks.
    for c in &cancelled {
        let dtstart  = local_to_utc(c.block_date, c.start_time, tz);
        let dtend    = local_to_utc(c.block_date, c.end_time,   tz);
        let summary  = escape_ics(&c.title);
        let sequence = c.cancelled_at.timestamp();
        let last_mod = c.cancelled_at.format("%Y%m%dT%H%M%SZ");

        ics.push_str(&format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}@clanplan\r\n\
             DTSTAMP:{dtstamp}\r\n\
             DTSTART:{dtstart}\r\n\
             DTEND:{dtend}\r\n\
             SUMMARY:{summary}\r\n\
             SEQUENCE:{sequence}\r\n\
             LAST-MODIFIED:{last_mod}\r\n\
             STATUS:CANCELLED\r\n\
             END:VEVENT\r\n",
            id = c.id,
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

async fn build_snapshot(state: &AppState, reunion_id: Uuid, tz: Tz) -> crate::error::AppResult<TodaySnapshot> {
    let today = Utc::now().with_timezone(&tz).date_naive();
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

/// Convert a naive local date+time in the given timezone to a UTC ICS datetime
/// string (`YYYYMMDDTHHMMSSz`).  Using UTC avoids the need for a `VTIMEZONE`
/// component (RFC 5545 §3.2.19).
///
/// DST ambiguity (fall-back clocks): `earliest()` picks the first occurrence.
/// DST gap (spring-forward, time doesn't exist): falls back to emitting the
/// local wall-clock digits with a `Z` suffix — wrong offset but valid syntax.
fn local_to_utc(date: chrono::NaiveDate, time: chrono::NaiveTime, tz: Tz) -> String {
    let naive_dt = date.and_time(time);
    naive_dt
        .and_local_timezone(tz)
        .earliest()
        .map(|dt| dt.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|| format!("{}Z", naive_dt.format("%Y%m%dT%H%M%S")))
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

    // local_to_utc with UTC timezone: output == input with Z suffix
    #[test]
    fn ics_datetime_format() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        let time = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert_eq!(local_to_utc(date, time, chrono_tz::UTC), "20260712T180000Z");
    }

    #[test]
    fn ics_datetime_midnight() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        let time = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
        assert_eq!(local_to_utc(date, time, chrono_tz::UTC), "20260712T000000Z");
    }

    // America/New_York is UTC-4 in July (EDT)
    #[test]
    fn ics_datetime_eastern_summer() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        let time = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert_eq!(
            local_to_utc(date, time, chrono_tz::America::New_York),
            "20260712T220000Z"  // 18:00 EDT = 22:00 UTC
        );
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
