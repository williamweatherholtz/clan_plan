// ── HTML page handlers (server-rendered with Askama) ─────────────────────────
//
// These routes live at human-friendly paths (/login, /dashboard, /reunions/:id,
// etc.) and render Askama templates.  The JSON API routes continue to live
// under /api/*.

use askama::Template;
use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts, Path, Query, State},
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form,
};
use chrono::{Datelike, NaiveDate};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    auth::{
        password as pwd,
        session::{get_or_create_csrf_token, save_user_id, validate_csrf, PENDING_INVITE_KEY, SESSION_USER_ID},
    },
    error::AppError,
    models::{
        activity::{ActivityIdea, ActivitySummary},
        app_settings::AppSettings,
        availability::Availability,
        expense::Expense,
        feedback::{SurveyQuestion, SurveyResponse},
        invite::{InviteMember, ReunionInvite},
        reunion::{Reunion, ReunionAdmin, ReunionDate, ReunionFamilyUnit},
        location::{LocationCandidate, VoteWithName},
        media::Media,
        schedule::{ScheduleBlock, Signup},
        user::{FamilyUnit, User},
    },
    phase::Phase,
    state::AppState,
};

use super::helpers;

// ── Embedded static assets ────────────────────────────────────────────────────

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

pub async fn serve_asset(Path(path): Path<String>) -> Response {
    match Assets::get(&path) {
        Some(content) => {
            let mime = match path.rsplit('.').next().unwrap_or("") {
                "css" => "text/css",
                "js" => "application/javascript",
                "png" => "image/png",
                "svg" => "image/svg+xml",
                "ico" => "image/x-icon",
                "woff2" => "font/woff2",
                _ => "application/octet-stream",
            };
            (
                [(header::CONTENT_TYPE, mime)],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ── Flash messages ────────────────────────────────────────────────────────────

const FLASH_KEY: &str = "flash";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlashMsg {
    pub kind: String,
    pub text: String,
}

async fn set_flash(session: &Session, kind: &str, text: impl Into<String>) {
    let _ = session
        .insert(FLASH_KEY, FlashMsg { kind: kind.into(), text: text.into() })
        .await;
}

async fn take_flash(session: &Session) -> Option<FlashMsg> {
    let msg: Option<FlashMsg> = session.get(FLASH_KEY).await.ok().flatten();
    if msg.is_some() {
        let _ = session.remove::<serde_json::Value>(FLASH_KEY).await;
    }
    msg
}

// ── Auth guard helpers ────────────────────────────────────────────────────────

/// Try to load the current user from session. Returns `None` if not logged in.
async fn current_user_opt(session: &Session, state: &AppState) -> Option<User> {
    let user_id: Uuid = session.get(SESSION_USER_ID).await.ok().flatten()?;
    User::find_by_id(state.db(), user_id).await.ok().filter(|u| u.is_active())
}

async fn require_login(session: &Session, state: &AppState) -> Result<User, Response> {
    current_user_opt(session, state)
        .await
        .ok_or_else(|| Redirect::to("/login").into_response())
}

async fn require_sysadmin(session: &Session, state: &AppState) -> Result<User, Response> {
    let user = require_login(session, state).await?;
    if !user.is_sysadmin() {
        return Err(Redirect::to("/dashboard").into_response());
    }
    Ok(user)
}

// ── Slug-or-UUID path extractor ──────────────────────────────────────────────
//
// Handles both `/reunions/:id/...` (direct UUID) and `/r/:slug/...` (slug →
// DB lookup) so every page handler can serve both URL shapes without change.

/// Bundles the four lookups every reunion page handler used to do by hand:
///   1. require_login(...)
///   2. SlugOrId resolution
///   3. load_reunion_for_member(...)
///   4. user_is_ra(...) + take_flash(...)
///
/// Use it as the first extractor in any reunion-scoped page handler to drop
/// 4 lines of boilerplate per handler. Backed by `tokio::try_join!` so the
/// independent admin/membership checks run in parallel.
pub struct ReunionPageContext {
    pub user: User,
    pub reunion: Reunion,
    pub is_ra: bool,
    pub flash: Option<FlashMsg>,
}

#[async_trait]
impl<S> FromRequestParts<S> for ReunionPageContext
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| Redirect::to("/login").into_response())?;
        let app_state = AppState::from_ref(state);
        let user = require_login(&session, &app_state).await?;
        let SlugOrId(reunion_id) = SlugOrId::from_request_parts(parts, state).await?;
        let reunion = helpers::load_reunion_for_member(&app_state, &user, reunion_id).await?;
        let flash = take_flash(&session).await;
        let is_ra = helpers::user_is_ra(&app_state, &user, reunion_id).await;
        Ok(ReunionPageContext { user, reunion, is_ra, flash })
    }
}

pub struct SlugOrId(pub Uuid);

#[async_trait]
impl<S> FromRequestParts<S> for SlugOrId
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        use std::collections::HashMap;
        let app_state = AppState::from_ref(state);
        let map = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map(|p| p.0)
            .unwrap_or_default();

        // /reunions/:id/... — direct UUID
        if let Some(id_str) = map.get("id") {
            if let Ok(id) = id_str.parse::<Uuid>() {
                return Ok(SlugOrId(id));
            }
        }
        // /r/:slug/... — slug lookup
        if let Some(slug) = map.get("slug") {
            if let Ok(r) = Reunion::find_by_slug(app_state.db(), slug).await {
                return Ok(SlugOrId(r.id));
            }
        }
        // Render a 404 instead of silently redirecting to /dashboard — the
        // silent redirect was confusing UX (looks like a session expiry).
        Err((
            axum::http::StatusCode::NOT_FOUND,
            "reunion not found — it may have been deleted or the URL is wrong",
        )
            .into_response())
    }
}

// ── Reunion tab helper ────────────────────────────────────────────────────────

pub struct NavTab {
    pub path: String,
    pub label: String,
    pub active: bool,
    /// 0 = top-level, 1 = planning/prep, 2 = during-reunion
    pub group: u8,
    /// True when the active tab belongs to this tab's group (highlights the dropdown button).
    pub group_has_active: bool,
}

/// Build the reunion sub-navigation.
/// `active_path` should match the tab's `path` field (e.g. `"activities"`).
/// `rules_label` is the per-reunion label for the rules pane (defaults to
/// "House Rules" in the schema, but each reunion can rename it).
fn reunion_tabs(_reunion_id: Uuid, active_path: &str, rules_label: &str) -> Vec<NavTab> {
    // (path, label, group)
    // group 0 = always-visible top-level tabs
    // group 1 = "Plan" dropdown (pre-day setup; not relevant once the
    //           reunion is live but still reachable for edits)
    //
    // Schedule and Today are deliberately not in the bar — Overview adapts
    // by phase and surfaces them. /schedule and /today still resolve, so
    // existing links keep working.
    let defs: &[(&str, String, u8)] = &[
        ("",              "Overview".to_string(),       0),
        ("activities",    "Activities".to_string(),     0),
        ("rules",         rules_label.to_string(),      0),
        // Plan dropdown
        ("availability",  "Dates".to_string(),          1),
        ("locations",     "Locations".to_string(),      1),
        ("expenses",      "Expenses".to_string(),       1),
        ("survey",        "Survey".to_string(),         1),
        ("media",         "Photos".to_string(),         1),
        ("settings",      "Settings".to_string(),       1),
    ];
    // Which group does the active tab belong to?
    let active_group = defs.iter()
        .find(|(path, _, _)| *path == active_path)
        .map(|(_, _, g)| *g);
    defs.iter()
        .map(|(path, label, group)| NavTab {
            path: path.to_string(),
            label: label.clone(),
            active: *path == active_path,
            group: *group,
            group_has_active: active_group == Some(*group),
        })
        .collect()
}

// ── Calendar month builder ────────────────────────────────────────────────────

pub struct CalendarMonth {
    pub name: String,
    pub weeks: Vec<[Option<NaiveDate>; 7]>,
}

fn build_calendar_months(start: NaiveDate, end: NaiveDate) -> Vec<CalendarMonth> {
    // Degenerate ranges silently produced a blank calendar; surface and bail
    // instead so a callsite bug doesn't quietly disappear into the UI.
    if start > end {
        tracing::warn!(
            ?start, ?end,
            "build_calendar_months called with start > end — returning empty"
        );
        return Vec::new();
    }
    let mut months = Vec::new();
    let mut cur = NaiveDate::from_ymd_opt(start.year(), start.month(), 1).unwrap();
    let end_month = NaiveDate::from_ymd_opt(end.year(), end.month(), 1).unwrap();

    while cur <= end_month {
        let name = cur.format("%B %Y").to_string();
        let mut weeks: Vec<[Option<NaiveDate>; 7]> = Vec::new();

        // Find the Monday on or before the 1st of the month (ISO week: Mon=0)
        let first_weekday = cur.weekday().num_days_from_monday() as i64;
        let mut day = cur - chrono::Duration::days(first_weekday);

        // How many days in this month?
        let days_in_month = {
            let next_month = if cur.month() == 12 {
                NaiveDate::from_ymd_opt(cur.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(cur.year(), cur.month() + 1, 1).unwrap()
            };
            (next_month - cur).num_days()
        };
        let last_day = cur + chrono::Duration::days(days_in_month - 1);

        while day <= last_day {
            let mut week = [None; 7];
            for i in 0..7 {
                if day.month() == cur.month() {
                    week[i] = Some(day);
                }
                day += chrono::Duration::days(1);
            }
            weeks.push(week);
        }

        months.push(CalendarMonth { name, weeks });

        // Advance to next month
        cur = if cur.month() == 12 {
            NaiveDate::from_ymd_opt(cur.year() + 1, 1, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(cur.year(), cur.month() + 1, 1).unwrap()
        };
    }
    months
}

// ── Schedule view types ───────────────────────────────────────────────────────

/// Slot view with `user_signed_up` pre-computed to avoid `&expr` in templates.
pub struct ScheduleSlotPageView {
    pub slot: crate::models::schedule::SignupSlot,
    pub signups: Vec<crate::models::schedule::Signup>,
    pub is_full: bool,
    pub user_signed_up: bool,
}

pub struct ScheduleBlockPageView {
    pub block: crate::models::schedule::ScheduleBlock,
    pub slots: Vec<ScheduleSlotPageView>,
    /// True when the requesting user may edit/delete this block (creator or RA).
    pub can_modify: bool,
    /// Comma-joined display names of users committed to make this meal.
    /// Empty when the block isn't a meal or no one has signed up.
    pub make_names_str: String,
    /// Comma-joined display names of users committed to clean up this meal.
    pub cleanup_names_str: String,
}

pub struct ScheduleDay {
    pub label: String,
    pub blocks: Vec<ScheduleBlockPageView>,
}

// ── Location view type ────────────────────────────────────────────────────────

pub struct LocationPageView {
    pub candidate: LocationCandidate,
    pub avg_score_str: String,
    pub vote_count: i64,
    pub my_vote_score: Option<i16>,
    pub my_vote_comment: Option<String>,
    /// Non-empty only when the requesting user is an RA.
    pub ra_votes: Vec<VoteWithName>,
    pub selected: bool,
}

// ── Activity view type ────────────────────────────────────────────────────────

pub struct ActivityPageView {
    pub idea: ActivityIdea,
    pub comment_count: i64,
    /// "I'm in" RSVP — used for non-meal activities.
    pub rsvp_count: i64,
    pub my_rsvp: bool,
    pub rsvp_names_str: String,
    /// "I'll make" — meal activities only.
    pub make_count: i64,
    pub my_make: bool,
    pub make_names_str: String,
    /// "I'll cleanup" — meal activities only.
    pub cleanup_count: i64,
    pub my_cleanup: bool,
    pub cleanup_names_str: String,
    pub proposed_by_name: String,
    pub proposed_by_family: Option<String>,
    /// True when the logged-in user originally proposed this idea.
    pub is_own_idea: bool,
}

// ── Expense view type ─────────────────────────────────────────────────────────

pub struct ExpensePageView {
    pub expense: Expense,
    pub paid_by_name: String,
    pub amount_str: String,
}

pub struct BalanceView {
    pub family_name: String,
    pub net_cents: i64,
    pub net_dollars: String,
}

pub struct FamilyUnitView {
    pub id: Uuid,
    pub name: String,
}

// ── Survey question view ──────────────────────────────────────────────────────

/// One of the current user's own responses — shown with edit/delete controls.
pub struct MyResponseView {
    pub id: Uuid,
    pub response_text: String,
}

/// One response as seen by the RA (includes respondent name, no edit controls).
pub struct SurveyResponseView {
    pub display_name: String,
    pub response_text: String,
}

pub struct SurveyQuestionView {
    pub question: SurveyQuestion,
    /// The current user's own responses (may be multiple).
    pub my_responses: Vec<MyResponseView>,
    /// All responses with names — populated only for RA.
    pub all_responses: Vec<SurveyResponseView>,
}

// ── RA user view ──────────────────────────────────────────────────────────────

pub struct UserWithRaStatus {
    pub id: Uuid,
    pub display_name: String,
    pub email: String,
    pub is_ra: bool,
}

// ── Storage stats view ────────────────────────────────────────────────────────

pub struct StorageStatsView {
    pub total_files: i64,
    pub total_mb: String,
}

// ── Invite view ───────────────────────────────────────────────────────────────

pub struct InviteWithUrl {
    pub id: Uuid,
    pub join_url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ============================================================================
// ── Template structs ─────────────────────────────────────────────────────────
// ============================================================================

#[derive(Template)]
#[template(path = "auth/login.html")]
struct LoginPage {
    flash: Option<FlashMsg>,
    google_enabled: bool,
}

#[derive(Template)]
#[template(path = "auth/register.html")]
struct RegisterPage {
    flash: Option<FlashMsg>,
    google_enabled: bool,
    registration_enabled: bool,
}

#[derive(Template)]
#[template(path = "auth/forgot_password.html")]
#[allow(dead_code)]
struct ForgotPasswordPage {
    flash: Option<FlashMsg>,
    google_enabled: bool,
}

#[derive(Template)]
#[template(path = "auth/reset_password.html")]
#[allow(dead_code)]
struct ResetPasswordPage {
    flash: Option<FlashMsg>,
    google_enabled: bool,
    token: String,
}

pub struct ReunionCardView {
    pub id: Uuid,
    pub title: String,
    pub phase_label: String,
    pub description: Option<String>,
    pub slug: Option<String>,
    pub ra_names: String,
}

#[derive(Template)]
#[template(path = "pages/dashboard.html")]
struct DashboardPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunions: Vec<ReunionCardView>,
    has_archived: bool,
}

#[derive(Template)]
#[template(path = "pages/profile.html")]
struct ProfilePage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    email: String,
    display_name: String,
    avatar_url: String,
    csrf_token: String,
}

#[derive(Template)]
#[template(path = "pages/reunion.html")]
struct ReunionOverviewPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    reunion_date: Option<ReunionDate>,
    is_ra: bool,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
    /// How many distinct members have submitted availability for this reunion.
    avail_response_count: i64,
    /// Total active+verified members (denominator for progress fractions).
    member_count: i64,
    /// Number of location candidates added so far.
    location_count: i64,
    /// Slug-aware base URL for this reunion (e.g. "/r/slug" or "/reunions/uuid").
    base_url: String,
    /// Comma-separated RA display names (empty string if none).
    ra_names: String,
    /// Display name of the selected location candidate, if one has been chosen.
    selected_location_name: Option<String>,
    /// Top 3 activity ideas (pinned first, then by interest score).
    top_activities: Vec<TopActivityPreview>,
    /// Total non-cancelled activity ideas (used for the "View all" link count).
    activity_total: i64,
}

pub struct TopActivityPreview {
    pub id: Uuid,
    pub title: String,
    pub category: String,
    pub status: String,
    pub comment_count: i64,
    pub rsvp_count: i64,
    pub my_rsvp: bool,
}

pub struct FamilyUnitWithEnrolled {
    pub id: Uuid,
    pub name: String,
    pub enrolled: bool,
}

#[derive(Template)]
#[template(path = "pages/settings.html")]
struct SettingsPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
    /// All family units annotated with whether they're enrolled in this reunion.
    family_units: Vec<FamilyUnitWithEnrolled>,
    /// All users annotated with whether they are currently an RA for this reunion.
    all_users_with_ra: Vec<UserWithRaStatus>,
    /// Active invite links for this reunion (RA-generated).
    invites: Vec<InviteWithUrl>,
    /// Members who joined via invite link and haven't been assigned to a family unit.
    invite_members: Vec<InviteMember>,
}

#[derive(Template)]
#[template(path = "pages/join.html")]
struct JoinPage {
    flash: Option<FlashMsg>,
    reunion_title: String,
    google_enabled: bool,
}

#[derive(Template)]
#[template(path = "pages/availability.html")]
struct AvailabilityPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    reunion_date: Option<ReunionDate>,
    my_dates_json: String,
    months: Vec<CalendarMonth>,
    editable: bool,
    is_ra: bool,
    /// JSON object mapping "YYYY-MM-DD" → member_count. Empty object for non-RAs.
    heatmap_json: String,
    /// Total respondents (denominator for heatmap colours).
    heatmap_total: i64,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
}

#[derive(Template)]
#[template(path = "pages/locations.html")]
struct LocationsPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    locations: Vec<LocationPageView>,
    votes_revealed: bool,
    can_vote: bool,
    is_ra: bool,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
}

#[derive(Template)]
#[template(path = "pages/schedule.html")]
struct SchedulePage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    reunion_date: Option<ReunionDate>,
    days: Vec<ScheduleDay>,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
}

#[derive(Template)]
#[template(path = "pages/today.html")]
struct TodayPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
}

#[derive(Template)]
#[template(path = "pages/activities.html")]
struct ActivitiesPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    reunion_date: Option<ReunionDate>,
    activities: Vec<ActivityPageView>,
    is_ra: bool,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
    default_activity_minutes: i32,
    /// Date-picker bounds extended 14 days past either side of the reunion;
    /// scheduling outside `reunion_date` but within the buffer prompts a JS
    /// confirm before submit. None when the reunion has no dates set yet.
    schedule_min_date: Option<String>,
    schedule_max_date: Option<String>,
}

#[derive(Template)]
#[template(path = "pages/media.html")]
struct MediaPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    media: Vec<Media>,
    can_delete_media: bool,
    /// Per-file upload ceiling exposed to the JS uploader so it can reject
    /// oversize files before any bytes leave the client. Sourced from
    /// config().max_upload_bytes.
    max_upload_bytes: u64,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
}

#[derive(Template)]
#[template(path = "pages/expenses.html")]
struct ExpensesPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    expenses: Vec<ExpensePageView>,
    balances: Vec<BalanceView>,
    members: Vec<User>,
    family_units: Vec<FamilyUnitView>,
    current_user_id: Uuid,
    is_ra: bool,
    expenses_confirmed: bool,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
}

#[derive(Template)]
#[template(path = "pages/survey.html")]
struct SurveyPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    questions: Vec<SurveyQuestionView>,
    is_ra: bool,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
}

#[derive(Template)]
#[template(path = "pages/rules.html")]
struct RulesPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    /// Pre-rendered, sanitized HTML from the markdown body. Empty string
    /// when the RA hasn't written any rules yet — template shows an empty
    /// state in that case.
    body_html: String,
    comments: Vec<crate::routes::rules::RulesCommentViewMine>,
    current_user_id: Uuid,
    is_ra: bool,
    tabs: Vec<NavTab>,
    tab_label: String,
}

pub struct ReunionAdminView {
    pub id: Uuid,
    pub title: String,
    pub phase_label: String,
    pub slug: Option<String>,
    pub ra_names: String,
}

#[derive(Template)]
#[template(path = "pages/admin.html")]
struct AdminPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    users: Vec<User>,
    family_units: Vec<FamilyUnit>,
    storage: StorageStatsView,
    reunions: Vec<ReunionAdminView>,
    registration_enabled: bool,
}

// ============================================================================
// ── Page handlers ─────────────────────────────────────────────────────────────
// ============================================================================

// ── GET / ─────────────────────────────────────────────────────────────────────

pub async fn index(session: Session, State(state): State<AppState>) -> impl IntoResponse {
    if current_user_opt(&session, &state).await.is_some() {
        Redirect::to("/dashboard").into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}

// ── GET /login ────────────────────────────────────────────────────────────────

pub async fn login_page(session: Session, State(state): State<AppState>) -> impl IntoResponse {
    // Already logged in → dashboard
    if current_user_opt(&session, &state).await.is_some() {
        return Redirect::to("/dashboard").into_response();
    }
    let flash = take_flash(&session).await;
    LoginPage {
        flash,
        google_enabled: state.config().google_oauth_enabled(),
    }
    .into_response()
}

// ── POST /login ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
}

pub async fn login_form(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    let result: Result<User, &str> = async {
        let user = User::find_by_email(state.db(), &form.email)
            .await
            .map_err(|_| "Internal error")?
            .ok_or("Invalid email or password")?;

        if !user.is_active() {
            return Err("Account is deactivated");
        }

        let hash = user.password_hash.as_deref().ok_or("Invalid email or password")?;
        let valid = pwd::verify_password(&form.password, hash).await;
        if !valid {
            return Err("Invalid email or password");
        }

        save_user_id(&session, user.id).await.map_err(|_| "Internal error")?;
        Ok(user)
    }
    .await;

    match result {
        Ok(user) => {
            // Check for a pending invite stored when the user visited /join/:token
            let pending: Option<String> = session.get(PENDING_INVITE_KEY).await.ok().flatten();
            if let Some(token) = pending {
                let _ = session.remove::<String>(PENDING_INVITE_KEY).await;
                if let Ok(invite) = ReunionInvite::find_by_token(state.db(), &token).await {
                    let _ = ReunionInvite::redeem(state.db(), &invite, user.id).await;
                    if let Ok(reunion) = Reunion::find_by_id(state.db(), invite.reunion_id).await {
                        let url = match &reunion.slug {
                            Some(s) => format!("/r/{}", s),
                            None => format!("/reunions/{}", reunion.id),
                        };
                        return Redirect::to(&url).into_response();
                    }
                }
            }
            Redirect::to("/dashboard").into_response()
        }
        Err(msg) => {
            set_flash(&session, "error", msg).await;
            Redirect::to("/login").into_response()
        }
    }
}

// ── GET /register ─────────────────────────────────────────────────────────────

pub async fn register_page(session: Session, State(state): State<AppState>) -> impl IntoResponse {
    let flash = take_flash(&session).await;
    let registration_enabled = AppSettings::get(state.db())
        .await
        .map(|s| s.registration_enabled)
        .unwrap_or(false);
    RegisterPage {
        flash,
        google_enabled: state.config().google_oauth_enabled(),
        registration_enabled,
    }
    .into_response()
}

// ── POST /register ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RegisterForm {
    display_name: String,
    email: String,
    password: String,
}

pub async fn register_form(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<RegisterForm>,
) -> impl IntoResponse {
    use crate::models::user::{EmailVerification, NewUser};

    let registration_enabled = AppSettings::get(state.db())
        .await
        .map(|s| s.registration_enabled)
        .unwrap_or(false);
    if !registration_enabled {
        set_flash(&session, "error", "Account registration is currently disabled.").await;
        return Redirect::to("/register").into_response();
    }

    if form.password.len() < 8 {
        set_flash(&session, "error", "Password must be at least 8 characters").await;
        return Redirect::to("/register").into_response();
    }
    if form.display_name.trim().is_empty() {
        set_flash(&session, "error", "Display name cannot be empty").await;
        return Redirect::to("/register").into_response();
    }
    if form.display_name.len() > 100 {
        set_flash(&session, "error", "Display name cannot exceed 100 characters").await;
        return Redirect::to("/register").into_response();
    }

    let result: Result<(), String> = async {
        let hash = pwd::hash_password(&form.password)
            .await
            .map_err(|_| "Registration failed".to_string())?;
        let user = crate::models::user::User::create(
            state.db(),
            NewUser {
                email: form.email.clone(),
                display_name: form.display_name.trim().to_string(),
                password_hash: Some(hash),
                google_id: None,
                family_unit_id: None,
                avatar_url: None,
            },
        )
        .await
        .map_err(|e| match e {
            AppError::Conflict(m) => m,
            _ => "Registration failed".to_string(),
        })?;

        // Send verification email
        let token = pwd::generate_token();
        EmailVerification::create(state.db(), user.id, &token)
            .await
            .map_err(|_| "Registration failed".to_string())?;
        let verify_url = format!("{}/api/auth/verify-email?token={}", state.config().app_base_url, token);
        let _ = state.mailer().send_verification_email(
            &user.email,
            &user.display_name,
            &verify_url,
        ).await;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            set_flash(&session, "success",
                "Account created! Please check your email to verify your address.").await;
            Redirect::to("/login").into_response()
        }
        Err(msg) => {
            set_flash(&session, "error", msg).await;
            Redirect::to("/register").into_response()
        }
    }
}

// ── GET /forgot-password ──────────────────────────────────────────────────────

pub async fn forgot_password_page(session: Session, State(state): State<AppState>) -> impl IntoResponse {
    let flash = take_flash(&session).await;
    ForgotPasswordPage {
        flash,
        google_enabled: state.config().google_oauth_enabled(),
    }
    .into_response()
}

// ── POST /forgot-password ─────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ForgotPasswordForm {
    email: String,
}

pub async fn forgot_password_form(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<ForgotPasswordForm>,
) -> impl IntoResponse {
    use crate::models::user::PasswordReset;

    // Always show success to prevent email enumeration
    let _ = async {
        let user = User::find_by_email(state.db(), &form.email).await?;
        if let Some(user) = user {
            if user.is_active() {
                let token = pwd::generate_token();
                PasswordReset::create(state.db(), user.id, &token).await?;
                let reset_url = format!("{}/reset-password?token={}", state.config().app_base_url, token);
                let _ = state.mailer().send_password_reset_email(
                    &user.email,
                    &user.display_name,
                    &reset_url,
                ).await;
            }
        }
        Ok::<_, AppError>(())
    }
    .await;

    set_flash(&session, "success",
        "If that email exists we've sent a reset link. Check your inbox.").await;
    Redirect::to("/forgot-password").into_response()
}

// ── GET /reset-password ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ResetPasswordQuery {
    token: String,
}

pub async fn reset_password_page(
    session: Session,
    Query(q): Query<ResetPasswordQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let flash = take_flash(&session).await;
    ResetPasswordPage {
        flash,
        google_enabled: state.config().google_oauth_enabled(),
        token: q.token,
    }
    .into_response()
}

// ── POST /reset-password ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ResetPasswordForm {
    token: String,
    password: String,
}

pub async fn reset_password_form(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<ResetPasswordForm>,
) -> impl IntoResponse {
    use crate::models::user::PasswordReset;

    if form.password.len() < 8 {
        set_flash(&session, "error", "Password must be at least 8 characters").await;
        let redir = format!("/reset-password?token={}", form.token);
        return Redirect::to(&redir).into_response();
    }

    let result: Result<(), &str> = async {
        let reset = PasswordReset::consume(state.db(), &form.token)
            .await
            .map_err(|_| "Invalid or expired reset token")?;
        let hash = pwd::hash_password(&form.password)
            .await
            .map_err(|_| "Failed to update password")?;
        User::update_password_hash(state.db(), reset.user_id, &hash)
            .await
            .map_err(|_| "Failed to update password")?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            set_flash(&session, "success", "Password updated. Please sign in.").await;
            Redirect::to("/login").into_response()
        }
        Err(msg) => {
            set_flash(&session, "error", msg).await;
            let redir = format!("/reset-password?token={}", form.token);
            Redirect::to(&redir).into_response()
        }
    }
}

// ── GET /dashboard ────────────────────────────────────────────────────────────

pub async fn dashboard(
    session: Session,
    State(state): State<AppState>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;

    // Single SQL call returns reunions the user can see (sysadmin sees all,
    // non-sysadmins see RA-of / family-unit-of / invited-to, with Draft
    // filtered out for non-RAs). Was 4 separate queries + Rust-side filter.
    let all_accessible = Reunion::list_accessible(
        state.db(),
        user.id,
        user.family_unit_id,
        user.is_sysadmin(),
    )
    .await?;
    let accessible: Vec<&Reunion> = all_accessible.iter().collect();

    // If the user has exactly one accessible reunion and it is not in Draft, go straight to it.
    // If there are also Draft-phase reunions visible (i.e. the user is an RA setting one up),
    // skip the redirect so the dashboard renders and they can see all their reunions.
    let non_draft: Vec<&&Reunion> = accessible.iter().filter(|r| r.phase != Phase::Draft).collect();
    if accessible.len() == 1 && non_draft.len() == 1 {
        let r = non_draft[0];
        let url = match &r.slug {
            Some(s) => format!("/r/{}", s),
            None => format!("/reunions/{}", r.id),
        };
        return Ok(Redirect::to(&url).into_response());
    }

    let flash = take_flash(&session).await;
    let has_archived = accessible.iter().any(|r| r.phase == Phase::Archived);

    // Load RA names in one query for card display.
    let admin_rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT ra.reunion_id, u.display_name
         FROM reunion_admins ra JOIN users u ON u.id = ra.user_id",
    )
    .fetch_all(state.db())
    .await
    .unwrap_or_else(|e| { tracing::warn!("pages.rs:{} db error (returning empty): {{e:?}}", line!()); Default::default() });

    let reunions: Vec<ReunionCardView> = accessible
        .into_iter()
        .map(|r| {
            let names: Vec<String> = admin_rows
                .iter()
                .filter(|(rid, _)| *rid == r.id)
                .map(|(_, name)| name.clone())
                .collect();
            let ra_names = if names.is_empty() { String::new() } else { names.join(", ") };
            ReunionCardView {
                id: r.id,
                title: r.title.clone(),
                phase_label: r.phase.label().to_string(),
                description: r.description.clone(),
                slug: r.slug.clone(),
                ra_names,
            }
        })
        .collect();

    Ok(DashboardPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        has_archived,
        reunions,
    }
    .into_response())
}

// ── GET /profile ──────────────────────────────────────────────────────────────

pub async fn profile_page(
    session: Session,
    State(state): State<AppState>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let flash = take_flash(&session).await;
    let csrf_token = get_or_create_csrf_token(&session).await;
    Ok(ProfilePage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        email: user.email.clone(),
        display_name: user.display_name.clone(),
        avatar_url: user.avatar_url.clone().unwrap_or_default(),
        csrf_token,
    }
    .into_response())
}

// ── POST /profile ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ProfileForm {
    display_name: String,
    avatar_url: String,
    csrf_token: String,
}

use crate::auth::is_allowed_avatar_url;

pub async fn profile_form(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<ProfileForm>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;

    if !validate_csrf(&session, &form.csrf_token).await {
        set_flash(&session, "error", "Invalid request. Please try again.").await;
        return Ok(Redirect::to("/profile").into_response());
    }

    if !form.display_name.trim().is_empty() {
        let _ = User::update_display_name(state.db(), user.id, form.display_name.trim()).await;
    }

    let avatar_trimmed = form.avatar_url.trim();
    if !avatar_trimmed.is_empty()
        && !is_allowed_avatar_url(avatar_trimmed)
    {
        set_flash(&session, "error", "Avatar URL must be a Google or GitHub profile image URL.").await;
        return Ok(Redirect::to("/profile").into_response());
    }
    let avatar = if avatar_trimmed.is_empty() { None } else { Some(avatar_trimmed) };
    let _ = User::set_avatar(state.db(), user.id, avatar).await;

    set_flash(&session, "success", "Profile updated.").await;
    Ok(Redirect::to("/profile").into_response())
}

// ── POST /profile/password ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ChangePasswordForm {
    current_password: String,
    new_password: String,
    csrf_token: String,
}

pub async fn change_password_form(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<ChangePasswordForm>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;

    if !validate_csrf(&session, &form.csrf_token).await {
        set_flash(&session, "error", "Invalid request. Please try again.").await;
        return Ok(Redirect::to("/profile").into_response());
    }

    let do_change = async {
        if form.new_password.len() < 8 {
            return Err("New password must be at least 8 characters".to_string());
        }
        let hash = user.password_hash.as_deref().ok_or_else(|| "No password set on this account".to_string())?;
        let valid = pwd::verify_password(&form.current_password, hash).await;
        if !valid {
            return Err("Current password is incorrect".to_string());
        }
        let new_hash = pwd::hash_password(&form.new_password)
            .await
            .map_err(|_| "Failed to update password".to_string())?;
        User::update_password_hash(state.db(), user.id, &new_hash)
            .await
            .map_err(|_| "Failed to update password".to_string())?;
        Ok(())
    };

    match do_change.await {
        Ok(()) => {
            set_flash(&session, "success", "Password updated.").await;
        }
        Err(msg) => {
            set_flash(&session, "error", msg).await;
        }
    }
    Ok(Redirect::to("/profile").into_response())
}

// ── GET /reunions/:id ─────────────────────────────────────────────────────────

pub async fn reunion_overview(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let mut reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;
    let reunion_date = ReunionDate::find_for_reunion(state.db(), reunion_id).await.ok().flatten();

    // Auto-activate: if PrepCompleted and the start date has arrived, advance to Active.
    if let Some(activated) = helpers::maybe_auto_activate(&state, &reunion).await {
        reunion = activated;
    }

    // Auto-redirect to Today view when the reunion is actively happening today.
    if reunion.phase == Phase::Active {
        if let Some(rd) = &reunion_date {
            let tz_str = helpers::get_reunion_tz_string(&state, &reunion).await;
            let tz: chrono_tz::Tz = tz_str.parse().unwrap_or(chrono_tz::UTC);
            let today = chrono::Utc::now().with_timezone(&tz).date_naive();
            if today >= rd.start_date && today <= rd.end_date {
                let today_url = match &reunion.slug {
                    Some(s) => format!("/r/{}/today", s),
                    None => format!("/reunions/{}/today", reunion_id),
                };
                return Ok(Redirect::to(&today_url).into_response());
            }
        }
    }

    let avail_response_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(DISTINCT user_id) FROM availability WHERE reunion_id = $1",
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(0);

    // Count verified users who are actually members of this reunion:
    // RAs, users in enrolled family units, and invite-link members.
    let member_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(DISTINCT u.id)
         FROM users u
         WHERE u.deactivated_at IS NULL
           AND u.email_verified_at IS NOT NULL
           AND (
             EXISTS(SELECT 1 FROM reunion_admins
                    WHERE reunion_id = $1 AND user_id = u.id)
             OR (u.family_unit_id IS NOT NULL AND EXISTS(
               SELECT 1 FROM reunion_family_units
               WHERE reunion_id = $1 AND family_unit_id = u.family_unit_id))
             OR EXISTS(SELECT 1 FROM reunion_invite_members
                       WHERE reunion_id = $1 AND user_id = u.id)
           )",
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(1)
    .max(1);

    let location_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM location_candidates WHERE reunion_id = $1",
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(0);

    let base_url = match &reunion.slug {
        Some(s) => format!("/r/{}", s),
        None => format!("/reunions/{}", reunion_id),
    };

    let ra_name_list: Vec<String> = sqlx::query_scalar(
        "SELECT u.display_name FROM reunion_admins ra JOIN users u ON u.id = ra.user_id \
         WHERE ra.reunion_id = $1 ORDER BY u.display_name",
    )
    .bind(reunion_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_else(|e| { tracing::warn!("pages.rs:{} db error (returning empty): {{e:?}}", line!()); Default::default() });
    let ra_names = ra_name_list.join(", ");

    let selected_location_name = if let Some(loc_id) = reunion.selected_location_id {
        LocationCandidate::find_by_id(state.db(), loc_id)
            .await
            .ok()
            .map(|loc| loc.title)
    } else {
        None
    };

    // Top 3 activity ideas for the overview preview block.
    // Pinned first, then by RSVP count, then newest.
    let top_activity_rows = sqlx::query_as::<
        _,
        (Uuid, String, String, String, i64, i64, bool),
    >(
        r#"
        SELECT ai.id,
               ai.title,
               ai.category,
               ai.status::text,
               COUNT(DISTINCT ac.id)                                                    AS comment_count,
               COUNT(DISTINCT ar.user_id) FILTER (WHERE ar.role = 'in')                 AS rsvp_count,
               COALESCE(BOOL_OR(ar.user_id = $2 AND ar.role = 'in'), FALSE)             AS my_rsvp
        FROM activity_ideas ai
        LEFT JOIN activity_comments ac ON ac.activity_idea_id = ai.id
        LEFT JOIN activity_rsvps    ar ON ar.activity_idea_id = ai.id
        WHERE ai.reunion_id = $1 AND ai.status != 'cancelled'
        GROUP BY ai.id
        ORDER BY (ai.status = 'pinned') DESC,
                 COUNT(DISTINCT ar.user_id) DESC,
                 ai.created_at DESC
        LIMIT 3
        "#,
    )
    .bind(reunion_id)
    .bind(user.id)
    .fetch_all(state.db())
    .await
    .unwrap_or_else(|e| { tracing::warn!("pages.rs:{} db error (returning empty): {{e:?}}", line!()); Default::default() });

    let top_activities: Vec<TopActivityPreview> = top_activity_rows
        .into_iter()
        .map(|(id, title, category, status, comments, rsvps, mine)| {
            TopActivityPreview {
                id,
                title,
                category,
                status,
                comment_count: comments,
                rsvp_count: rsvps,
                my_rsvp: mine,
            }
        })
        .collect();

    let activity_total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM activity_ideas WHERE reunion_id = $1 AND status != 'cancelled'",
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(0);

    Ok(ReunionOverviewPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "", &reunion.rules_label),
        tab_label: "Overview",
        reunion,
        reunion_date,
        is_ra,
        avail_response_count,
        member_count,
        location_count,
        base_url,
        ra_names,
        selected_location_name,
        top_activities,
        activity_total,
    }
    .into_response())
}

// ── GET /reunions/:id/availability ────────────────────────────────────────────

pub async fn availability_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;

    let my_dates = Availability::for_user(state.db(), reunion_id, user.id)
        .await?;
    let my_dates_json = serde_json::to_string(
        &my_dates.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());

    // Determine date range to show:
    //   1. RA-set poll window (avail_poll_start/end on the reunion row)
    //   2. Confirmed reunion dates
    //   3. Fallback: today + 90 days
    let reunion_date = ReunionDate::find_for_reunion(state.db(), reunion_id)
        .await
        .ok()
        .flatten();
    let (start, end) = match (reunion.avail_poll_start, reunion.avail_poll_end) {
        (Some(s), Some(e)) => (s, e),
        _ => match &reunion_date {
            Some(d) => (d.start_date, d.end_date),
            None => {
                let today = chrono::Local::now().date_naive();
                (today, today + chrono::Duration::days(90))
            }
        },
    };
    let months = build_calendar_months(start, end);

    let editable = matches!(reunion.phase, Phase::Availability);

    // Heatmap (RA only) — build a JSON map {date: count} for the template
    let (heatmap_json, heatmap_total) = if is_ra {
        let total = Availability::respondent_count(state.db(), reunion_id)
            .await
            .unwrap_or(0);
        let entries = Availability::heatmap(state.db(), reunion_id)
            .await?;
        let map: std::collections::HashMap<String, i64> = entries
            .into_iter()
            .map(|e| (e.available_date.format("%Y-%m-%d").to_string(), e.member_count))
            .collect();
        // HashMap<String, i64> always serializes — make a future refactor
        // that introduces non-string keys fail loudly instead of silently
        // returning "{}" to the template.
        (serde_json::to_string(&map).expect("HashMap<String,i64> JSON serialization is infallible"), total)
    } else {
        ("{}".into(), 0)
    };

    Ok(AvailabilityPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "availability", &reunion.rules_label),
        tab_label: "Availability",
        reunion,
        reunion_date,
        my_dates_json,
        months,
        editable,
        is_ra,
        heatmap_json,
        heatmap_total,
    }
    .into_response())
}

// ── GET /reunions/:id/locations ───────────────────────────────────────────────

pub async fn locations_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    use crate::models::location::LocationVote;

    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;
    let votes_revealed = reunion.location_votes_revealed;
    // Voting is open from Availability onward (parallel with the dates poll).
    // RA can always vote.
    let can_vote = is_ra
        || matches!(
            reunion.phase,
            Phase::Availability | Phase::Locations | Phase::PrepCompleted | Phase::Active
        );

    let candidates = LocationCandidate::list_for_reunion(state.db(), reunion_id)
        .await?;

    // Bulk-fetch aggregates, my-vote, and ra-votes for the entire reunion's
    // candidate set in 2 (or 3, if RA) round trips instead of 2N (or 3N).
    let aggregates: Vec<(Uuid, Option<f64>, i64)> = sqlx::query_as(
        "SELECT lc.id, AVG(lv.score::float), COUNT(lv.score)
         FROM location_candidates lc
         LEFT JOIN location_votes lv ON lv.location_candidate_id = lc.id
         WHERE lc.reunion_id = $1
         GROUP BY lc.id",
    )
    .bind(reunion_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    let aggregate_map: std::collections::HashMap<Uuid, (Option<f64>, i64)> = aggregates
        .into_iter()
        .map(|(id, avg, count)| (id, (avg, count)))
        .collect();

    let my_votes: Vec<(Uuid, i16, Option<String>)> = sqlx::query_as(
        "SELECT lv.location_candidate_id, lv.score, lv.comment
         FROM location_votes lv
         JOIN location_candidates lc ON lc.id = lv.location_candidate_id
         WHERE lc.reunion_id = $1 AND lv.user_id = $2",
    )
    .bind(reunion_id)
    .bind(user.id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    let my_vote_map: std::collections::HashMap<Uuid, (i16, Option<String>)> =
        my_votes.into_iter().map(|(id, s, c)| (id, (s, c))).collect();

    let mut ra_votes_map: std::collections::HashMap<Uuid, Vec<crate::models::location::VoteWithName>> =
        std::collections::HashMap::new();
    if is_ra {
        let rows: Vec<(Uuid, String, i16, Option<String>)> = sqlx::query_as(
            "SELECT lv.location_candidate_id, u.display_name, lv.score, lv.comment
             FROM location_votes lv
             JOIN users u ON u.id = lv.user_id
             JOIN location_candidates lc ON lc.id = lv.location_candidate_id
             WHERE lc.reunion_id = $1
             ORDER BY lv.score DESC, u.display_name",
        )
        .bind(reunion_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default();
        for (cand_id, display_name, score, comment) in rows {
            ra_votes_map
                .entry(cand_id)
                .or_default()
                .push(crate::models::location::VoteWithName {
                    display_name,
                    score,
                    comment,
                });
        }
    }

    let mut locations = Vec::new();
    for c in candidates {
        let (avg_score, vote_count) = aggregate_map.get(&c.id).cloned().unwrap_or((None, 0));
        let avg_score_str = if votes_revealed {
            avg_score.map(|v| format!("{:.1}", v)).unwrap_or_default()
        } else {
            String::new()
        };
        let selected = reunion.selected_location_id == Some(c.id);
        let my_vote = my_vote_map.get(&c.id).cloned();
        let my_vote_score = my_vote.as_ref().map(|(s, _)| *s);
        let my_vote_comment = my_vote.and_then(|(_, c)| c);
        let ra_votes = ra_votes_map.remove(&c.id).unwrap_or_default();
        locations.push(LocationPageView {
            candidate: c,
            avg_score_str,
            vote_count,
            my_vote_score,
            my_vote_comment,
            ra_votes,
            selected,
        });
    }

    Ok(LocationsPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "locations", &reunion.rules_label),
        tab_label: "Locations",
        reunion,
        locations,
        votes_revealed,
        can_vote,
        is_ra,
    }
    .into_response())
}

// ── GET /reunions/:id/schedule ────────────────────────────────────────────────

pub async fn schedule_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    use crate::models::schedule::SignupSlot;

    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;
    let reunion_date = ReunionDate::find_for_reunion(state.db(), reunion_id).await.ok().flatten();
    let blocks = ScheduleBlock::list_for_reunion(state.db(), reunion_id)
        .await?;

    // User's signup slot IDs — fetched once, used to annotate each slot
    let user_signup_slot_ids: std::collections::HashSet<Uuid> =
        Signup::list_for_user_in_reunion(state.db(), reunion_id, user.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.signup_slot_id)
            .collect();

    // Bulk-fetch every slot and every signup for the whole reunion so the
    // per-block / per-slot loops below do zero DB work. Was 1 + B*(1+S)
    // round trips (B blocks each with S slots); now 2 regardless of B/S.
    let all_slots: Vec<SignupSlot> = sqlx::query_as(
        "SELECT s.*
         FROM signup_slots s
         JOIN schedule_blocks b ON b.id = s.schedule_block_id
         WHERE b.reunion_id = $1
         ORDER BY s.created_at",
    )
    .bind(reunion_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    let mut slots_by_block: std::collections::HashMap<Uuid, Vec<SignupSlot>> =
        std::collections::HashMap::new();
    for s in all_slots {
        slots_by_block.entry(s.schedule_block_id).or_default().push(s);
    }

    let all_signups: Vec<Signup> = sqlx::query_as(
        "SELECT sg.*
         FROM signups sg
         JOIN signup_slots ss ON ss.id = sg.signup_slot_id
         JOIN schedule_blocks b ON b.id = ss.schedule_block_id
         WHERE b.reunion_id = $1
         ORDER BY sg.created_at",
    )
    .bind(reunion_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    let mut signups_by_slot: std::collections::HashMap<Uuid, Vec<Signup>> =
        std::collections::HashMap::new();
    for sg in all_signups {
        signups_by_slot.entry(sg.signup_slot_id).or_default().push(sg);
    }

    // Build page view blocks with user_signed_up per slot
    let mut days: Vec<ScheduleDay> = Vec::new();
    for block in blocks {
        let slots_raw = slots_by_block.remove(&block.id).unwrap_or_default();
        let mut slot_views = Vec::new();
        for slot in slots_raw {
            let signups = signups_by_slot.remove(&slot.id).unwrap_or_default();
            let signup_count = signups.len() as i32;
            let is_full = slot.max_count.map(|m| signup_count >= m).unwrap_or(false);
            let user_signed_up = user_signup_slot_ids.contains(&slot.id);
            slot_views.push(ScheduleSlotPageView { slot, signups, is_full, user_signed_up });
        }
        let label = block.block_date.format("%A, %B %-d").to_string();
        let can_modify = is_ra || block.created_by == user.id;
        let (make_names, cleanup_names) =
            helpers::meal_rsvp_names_for_block(state.db(), block.id).await;
        let make_names_str = make_names.join(", ");
        let cleanup_names_str = cleanup_names.join(", ");
        let block_view = ScheduleBlockPageView {
            block,
            slots: slot_views,
            can_modify,
            make_names_str,
            cleanup_names_str,
        };
        if let Some(day) = days.iter_mut().find(|d| d.label == label) {
            day.blocks.push(block_view);
        } else {
            days.push(ScheduleDay { label, blocks: vec![block_view] });
        }
    }

    Ok(SchedulePage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "schedule", &reunion.rules_label),
        tab_label: "Schedule",
        reunion,
        reunion_date,
        days,
    }
    .into_response())
}

// ── GET /reunions/:id/today ───────────────────────────────────────────────────

pub async fn today_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    Ok(TodayPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "today", &reunion.rules_label),
        tab_label: "Today",
        reunion,
    }
    .into_response())
}

// ── GET /reunions/:id/activities ──────────────────────────────────────────────

pub async fn activities_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;
    let reunion_date = ReunionDate::find_for_reunion(state.db(), reunion_id)
        .await
        .ok()
        .flatten();

    let ideas = ActivityIdea::list_for_reunion(state.db(), reunion_id)
        .await?;

    // ── Bulk-fetch what the per-idea loop used to fetch one-at-a-time.
    //    Previously: 1 + (N × 3) queries — comment summary, rsvp rows, and
    //    proposer per idea. Now: 1 + 3 queries total, regardless of N.

    let summaries = ActivityIdea::summaries_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();
    let summary_map: std::collections::HashMap<uuid::Uuid, i64> = summaries
        .into_iter()
        .map(|s| (s.idea_id, s.comment_count))
        .collect();

    let all_rsvps: Vec<(uuid::Uuid, uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT ar.activity_idea_id, ar.user_id, u.display_name, ar.role
         FROM activity_rsvps ar
         JOIN users u           ON u.id  = ar.user_id
         JOIN activity_ideas ai ON ai.id = ar.activity_idea_id
         WHERE ai.reunion_id = $1
         ORDER BY u.display_name",
    )
    .bind(reunion_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    let mut rsvps_by_idea: std::collections::HashMap<uuid::Uuid, Vec<(uuid::Uuid, String, String)>> =
        std::collections::HashMap::new();
    for (idea_id, uid, name, role) in all_rsvps {
        rsvps_by_idea
            .entry(idea_id)
            .or_default()
            .push((uid, name, role));
    }

    let proposer_rows: Vec<(uuid::Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT DISTINCT u.id, u.display_name, f.name
         FROM users u
         LEFT JOIN family_units f ON f.id = u.family_unit_id
         WHERE u.id IN (SELECT DISTINCT proposed_by FROM activity_ideas WHERE reunion_id = $1)",
    )
    .bind(reunion_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    let proposer_map: std::collections::HashMap<uuid::Uuid, (String, Option<String>)> =
        proposer_rows
            .into_iter()
            .map(|(id, name, family)| (id, (name, family)))
            .collect();

    let mut activities = Vec::new();
    for idea in ideas {
        let comment_count = summary_map.get(&idea.id).copied().unwrap_or(0);
        let empty_rsvps: Vec<(uuid::Uuid, String, String)> = Vec::new();
        let rsvp_rows = rsvps_by_idea.get(&idea.id).unwrap_or(&empty_rsvps);
        let names_for = |role: &str| -> Vec<&str> {
            rsvp_rows
                .iter()
                .filter(|(_, _, r)| r == role)
                .map(|(_, n, _)| n.as_str())
                .collect()
        };
        let in_names = names_for("in");
        let make_names = names_for("make");
        let cleanup_names = names_for("cleanup");
        let rsvp_count = in_names.len() as i64;
        let my_rsvp = rsvp_rows
            .iter()
            .any(|(uid, _, r)| *uid == user.id && r == "in");
        let rsvp_names_str = in_names.join(", ");
        let make_count = make_names.len() as i64;
        let my_make = rsvp_rows
            .iter()
            .any(|(uid, _, r)| *uid == user.id && r == "make");
        let make_names_str = make_names.join(", ");
        let cleanup_count = cleanup_names.len() as i64;
        let my_cleanup = rsvp_rows
            .iter()
            .any(|(uid, _, r)| *uid == user.id && r == "cleanup");
        let cleanup_names_str = cleanup_names.join(", ");
        let proposer = proposer_map
            .get(&idea.proposed_by)
            .cloned()
            .unwrap_or_else(|| ("Unknown".to_string(), None));
        let is_own_idea = idea.proposed_by == user.id;
        activities.push(ActivityPageView {
            idea,
            comment_count,
            rsvp_count,
            my_rsvp,
            rsvp_names_str,
            make_count,
            my_make,
            make_names_str,
            cleanup_count,
            my_cleanup,
            cleanup_names_str,
            proposed_by_name: proposer.0,
            proposed_by_family: proposer.1,
            is_own_idea,
        });
    }

    let default_activity_minutes = reunion.default_activity_duration_minutes;
    let (schedule_min_date, schedule_max_date) = match &reunion_date {
        Some(rd) => (
            Some((rd.start_date - chrono::Duration::days(14)).to_string()),
            Some((rd.end_date + chrono::Duration::days(14)).to_string()),
        ),
        None => (None, None),
    };
    Ok(ActivitiesPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "activities", &reunion.rules_label),
        tab_label: "Activities",
        reunion,
        reunion_date,
        activities,
        is_ra,
        default_activity_minutes,
        schedule_min_date,
        schedule_max_date,
    }
    .into_response())
}

// ── GET /reunions/:id/media ───────────────────────────────────────────────────

pub async fn media_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;

    let media = Media::list_for_reunion(state.db(), reunion_id)
        .await?;

    Ok(MediaPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "media", &reunion.rules_label),
        tab_label: "Photos",
        reunion,
        media,
        can_delete_media: is_ra || user.is_sysadmin(),
        max_upload_bytes: state.config().max_upload_bytes,
    }
    .into_response())
}

// ── GET /reunions/:id/expenses ────────────────────────────────────────────────

pub async fn expenses_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;

    let expense_list = Expense::list_for_reunion(state.db(), reunion_id)
        .await?;
    let all_users = User::list_all(state.db()).await?;

    let expenses = expense_list
        .into_iter()
        .map(|e| {
            let paid_by_name = all_users
                .iter()
                .find(|u| u.id == e.paid_by_user_id)
                .map(|u| u.display_name.clone())
                .unwrap_or_else(|| e.paid_by_user_id.to_string());
            let amount_str = format!("${}", crate::models::expense::format_cents(e.amount_cents as i64));
            ExpensePageView { expense: e, paid_by_name, amount_str }
        })
        .collect();

    let balance_data = Expense::balances_for_reunion(state.db(), reunion_id)
        .await?;

    // Load family units enrolled in this reunion — used both for resolving
    // balance row names and for the split_among payload in the Add modal.
    let participating_unit_ids =
        crate::models::reunion::ReunionFamilyUnit::list_ids_for_reunion(state.db(), reunion_id)
            .await?;
    let all_units = crate::models::user::FamilyUnit::list_all(state.db())
        .await?;
    let family_units: Vec<FamilyUnitView> = participating_unit_ids
        .iter()
        .filter_map(|id| all_units.iter().find(|u| u.id == *id))
        .map(|u| FamilyUnitView { id: u.id, name: u.name.clone() })
        .collect();

    let balances = balance_data
        .into_iter()
        .map(|b| {
            let family_name = all_units
                .iter()
                .find(|u| u.id == b.family_unit_id)
                .map(|u| u.name.clone())
                .unwrap_or_else(|| b.family_unit_id.to_string());
            let net_dollars = crate::models::expense::format_cents(b.net_cents);
            BalanceView { family_name, net_cents: b.net_cents, net_dollars }
        })
        .collect();

    let expenses_confirmed = sqlx::query_scalar::<_, bool>(
        "SELECT COUNT(*) > 0 FROM expense_confirmations WHERE reunion_id = $1 AND user_id = $2",
    )
    .bind(reunion_id)
    .bind(user.id)
    .fetch_one(state.db())
    .await
    .unwrap_or(false);

    Ok(ExpensesPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "expenses", &reunion.rules_label),
        tab_label: "Expenses",
        reunion,
        expenses,
        balances,
        members: all_users,
        family_units,
        current_user_id: user.id,
        is_ra,
        expenses_confirmed,
    }
    .into_response())
}

// ── GET /reunions/:id/survey ──────────────────────────────────────────────────

pub async fn survey_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {

    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;

    let qs = SurveyQuestion::list_for_reunion(state.db(), reunion_id)
        .await?;
    // Current user's own responses (may be multiple per question)
    let own_responses = SurveyResponse::list_for_user(state.db(), reunion_id, user.id)
        .await?;
    // All responses with names — RA only
    let named_responses = if is_ra {
        SurveyResponse::list_for_reunion_with_names(state.db(), reunion_id)
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };

    let questions = qs
        .into_iter()
        .map(|q| {
            let my_responses = own_responses
                .iter()
                .filter(|r| r.survey_question_id == q.id)
                .map(|r| MyResponseView {
                    id: r.id,
                    response_text: r.response_text.clone(),
                })
                .collect();
            let all_responses = named_responses
                .iter()
                .filter(|r| r.survey_question_id == q.id)
                .map(|r| SurveyResponseView {
                    display_name: r.display_name.clone(),
                    response_text: r.response_text.clone(),
                })
                .collect();
            SurveyQuestionView { question: q, my_responses, all_responses }
        })
        .collect();

    Ok(SurveyPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "survey", &reunion.rules_label),
        tab_label: "Survey",
        reunion,
        questions,
        is_ra,
    }
    .into_response())
}

// ── GET /reunions/:id/rules ───────────────────────────────────────────────────

pub async fn rules_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;

    let body_html = crate::routes::rules::render_markdown(reunion.rules_body.as_deref());
    let comments =
        crate::routes::rules::enriched_comments_for_render(&state, reunion_id, user.id)
            .await
            .unwrap_or_default();

    let tab_label = reunion.rules_label.clone();
    Ok(RulesPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "rules", &reunion.rules_label),
        tab_label,
        reunion,
        body_html,
        comments,
        current_user_id: user.id,
        is_ra,
    }
    .into_response())
}

// ── GET /reunions/:id/settings ────────────────────────────────────────────────

pub async fn settings_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;
    // Only RA or sysadmin may access settings
    if !is_ra && !user.is_sysadmin() {
        return Err(Redirect::to(&format!("/reunions/{}", reunion_id)).into_response());
    }
    let flash = take_flash(&session).await;
    let raw_family_units = FamilyUnit::list_all(state.db()).await?;
    let enrolled_ids = ReunionFamilyUnit::list_ids_for_reunion(state.db(), reunion_id)
        .await?;
    let family_units: Vec<FamilyUnitWithEnrolled> = raw_family_units
        .into_iter()
        .map(|fu| {
            let enrolled = enrolled_ids.contains(&fu.id);
            FamilyUnitWithEnrolled { id: fu.id, name: fu.name, enrolled }
        })
        .collect();
    let ra_ids = ReunionAdmin::list_ids_for_reunion(state.db(), reunion_id)
        .await?;
    let all_users_raw = User::list_all(state.db()).await?;
    let all_users_with_ra: Vec<UserWithRaStatus> = all_users_raw
        .into_iter()
        .map(|u| {
            let is_ra = ra_ids.contains(&u.id);
            UserWithRaStatus { id: u.id, display_name: u.display_name, email: u.email, is_ra }
        })
        .collect();

    let invites_raw = ReunionInvite::list_for_reunion(state.db(), reunion_id)
        .await?;
    let base_url = &state.config().app_base_url;
    let invites: Vec<InviteWithUrl> = invites_raw
        .into_iter()
        .map(|inv| {
            let join_url = format!("{}/join/{}", base_url, inv.token);
            InviteWithUrl { id: inv.id, join_url, created_at: inv.created_at }
        })
        .collect();
    let invite_members = ReunionInvite::list_unassigned_members(state.db(), reunion_id)
        .await?;

    Ok(SettingsPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "settings", &reunion.rules_label),
        tab_label: "Settings",
        reunion,
        family_units,
        all_users_with_ra,
        invites,
        invite_members,
    }
    .into_response())
}

// ── GET /admin ────────────────────────────────────────────────────────────────

pub async fn admin_page(
    session: Session,
    State(state): State<AppState>,
) -> Result<Response, Response> {
    let admin = require_sysadmin(&session, &state).await?;
    let flash = take_flash(&session).await;

    let users = User::list_all(state.db()).await?;
    let family_units = FamilyUnit::list_all(state.db()).await?;

    // Postgres SUM(BIGINT) returns NUMERIC, not BIGINT — sqlx silently failed
    // to deserialize into i64 and the unwrap_or below masked it as 0/0. The
    // ::BIGINT cast on the sum (and COALESCE for the all-NULL case when the
    // table is empty) keeps the row decodable into (i64, i64).
    let (total_bytes, total_files): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(file_size_bytes), 0)::BIGINT, COUNT(*) FROM media",
    )
    .fetch_one(state.db())
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("admin storage stats query failed: {e:?}");
        (0, 0)
    });

    let total_mb = format!("{:.1}", total_bytes as f64 / 1_048_576.0);
    let storage = StorageStatsView { total_files, total_mb };

    let all_reunions = Reunion::list_all(state.db()).await?;

    // Load all reunion_admins in one query
    let all_admin_rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT ra.reunion_id, ra.user_id, u.display_name
         FROM reunion_admins ra JOIN users u ON u.id = ra.user_id"
    )
    .fetch_all(state.db())
    .await
    .unwrap_or_else(|e| { tracing::warn!("pages.rs:{} db error (returning empty): {{e:?}}", line!()); Default::default() });

    let reunions: Vec<ReunionAdminView> = all_reunions
        .into_iter()
        .map(|r| {
            let ra_names: Vec<String> = all_admin_rows
                .iter()
                .filter(|(rid, _, _)| *rid == r.id)
                .map(|(_, _, name)| name.clone())
                .collect();
            let ra_names_str = if ra_names.is_empty() { "Unassigned".into() } else { ra_names.join(", ") };
            ReunionAdminView {
                id: r.id,
                title: r.title,
                phase_label: r.phase.label().to_string(),
                slug: r.slug,
                ra_names: ra_names_str,
            }
        })
        .collect();

    let registration_enabled = AppSettings::get(state.db())
        .await
        .map(|s| s.registration_enabled)
        .unwrap_or(false);

    Ok(AdminPage {
        user_name: admin.display_name.clone(),
        is_sysadmin: true,
        flash,
        users,
        family_units,
        storage,
        reunions,
        registration_enabled,
    }
    .into_response())
}

// ── GET /join/:token ──────────────────────────────────────────────────────────

pub async fn join_page(
    session: Session,
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> impl IntoResponse {
    // Validate the token first.
    let invite = match ReunionInvite::find_by_token(state.db(), &token).await {
        Ok(inv) => inv,
        Err(_) => {
            set_flash(&session, "error", "This invite link is invalid or has expired.").await;
            return Redirect::to("/login").into_response();
        }
    };
    let reunion = match Reunion::find_by_id(state.db(), invite.reunion_id).await {
        Ok(r) => r,
        Err(_) => {
            set_flash(&session, "error", "This invite link is invalid or has expired.").await;
            return Redirect::to("/login").into_response();
        }
    };

    // If already logged in, redeem immediately and redirect to the reunion.
    if let Some(user) = current_user_opt(&session, &state).await {
        let _ = ReunionInvite::redeem(state.db(), &invite, user.id).await;
        let url = match &reunion.slug {
            Some(s) => format!("/r/{}", s),
            None => format!("/reunions/{}", reunion.id),
        };
        return Redirect::to(&url).into_response();
    }

    // Not logged in — persist the token in the session so login_form can redeem it.
    let _ = session.insert(PENDING_INVITE_KEY, &token).await;
    let flash = take_flash(&session).await;
    JoinPage {
        flash,
        reunion_title: reunion.title.clone(),
        google_enabled: state.config().google_oauth_enabled(),
    }
    .into_response()
}
