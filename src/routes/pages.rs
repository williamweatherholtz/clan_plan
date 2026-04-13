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
        session::{get_or_create_csrf_token, save_user_id, validate_csrf, SESSION_USER_ID},
    },
    error::AppError,
    models::{
        activity::{ActivityIdea, ActivitySummary},
        announcement::Announcement,
        app_settings::AppSettings,
        availability::Availability,
        expense::Expense,
        feedback::{SurveyQuestion, SurveyResponse},
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
        Err(Redirect::to("/dashboard").into_response())
    }
}

// ── Reunion tab helper ────────────────────────────────────────────────────────

pub struct NavTab {
    pub path: String,
    pub label: &'static str,
    pub active: bool,
    /// 0 = top-level, 1 = planning/prep, 2 = during-reunion
    pub group: u8,
    /// True when the active tab belongs to this tab's group (highlights the dropdown button).
    pub group_has_active: bool,
}

/// Build the reunion sub-navigation.
/// `active_path` should match the tab's `path` field (e.g. `"activities"`).
fn reunion_tabs(_reunion_id: Uuid, active_path: &str) -> Vec<NavTab> {
    // (path, label, group)
    let defs: &[(&str, &str, u8)] = &[
        ("",              "Overview",      0),
        ("settings",      "Settings",      0),
        // Planning / prep
        ("availability",  "Availability",  1),
        ("locations",     "Locations",     1),
        ("expenses",      "Expenses",      1),
        ("survey",        "Survey",        1),
        // During / always-on
        ("today",         "Today",         2),
        ("activities",    "Activities",    2),
        ("schedule",      "Schedule",      2),
        ("media",         "Photos",        2),
        ("announcements", "Announcements", 2),
    ];
    // Which group does the active tab belong to?
    let active_group = defs.iter()
        .find(|(path, _, _)| *path == active_path)
        .map(|(_, _, g)| *g);
    defs.iter()
        .map(|(path, label, group)| NavTab {
            path: path.to_string(),
            label,
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

// ── Heatmap view type ─────────────────────────────────────────────────────────

pub struct HeatmapRow {
    pub date: String,
    pub count: i64,
    pub total: i64,
    pub pct: f64,
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
    pub avg_interest_str: String,
    pub vote_count: i64,
    pub comment_count: i64,
    pub my_vote: Option<i16>,
    pub rsvp_count: i64,
    pub my_rsvp: bool,
    /// Comma-separated display names of all members who marked "I'm in".
    pub rsvp_names_str: String,
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
    pub user_name: String,
    pub net_cents: i64,
    pub net_dollars: String,
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
    announcements: Vec<Announcement>,
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
    reunion_date: Option<ReunionDate>,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
    /// All family units annotated with whether they're enrolled in this reunion.
    family_units: Vec<FamilyUnitWithEnrolled>,
    /// All users annotated with whether they are currently an RA for this reunion.
    all_users_with_ra: Vec<UserWithRaStatus>,
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
    heatmap: Vec<HeatmapRow>,
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
#[template(path = "pages/announcements.html")]
struct AnnouncementsPage {
    user_name: String,
    is_sysadmin: bool,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    announcements: Vec<Announcement>,
    is_ra: bool,
    tabs: Vec<NavTab>,
    tab_label: &'static str,
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
    let result: Result<(), &str> = async {
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
        Ok(())
    }
    .await;

    match result {
        Ok(()) => Redirect::to("/dashboard").into_response(),
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

    // Determine which reunions this user can access.
    // Load membership data in two efficient queries.
    let user_ra_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT reunion_id FROM reunion_admins WHERE user_id = $1",
    )
    .bind(user.id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

    let user_enrolled_ids: Vec<Uuid> = if let Some(fu_id) = user.family_unit_id {
        sqlx::query_scalar(
            "SELECT reunion_id FROM reunion_family_units WHERE family_unit_id = $1",
        )
        .bind(fu_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    } else {
        vec![]
    };

    let all = Reunion::list_all(state.db()).await.unwrap_or_default();

    // Filter to reunions accessible to this user.
    let accessible: Vec<&Reunion> = all.iter().filter(|r| {
        if user.is_sysadmin() { return true; }
        let is_ra = user_ra_ids.contains(&r.id);
        if r.phase == Phase::Draft { is_ra } else { is_ra || user_enrolled_ids.contains(&r.id) }
    }).collect();

    // If exactly one non-Draft accessible reunion, go straight to it.
    let non_draft: Vec<&&Reunion> = accessible.iter().filter(|r| r.phase != Phase::Draft).collect();
    if non_draft.len() == 1 {
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
    .unwrap_or_default();

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

const ALLOWED_AVATAR_PREFIXES: &[&str] = &[
    "https://lh3.googleusercontent.com/",
    "https://avatars.githubusercontent.com/",
];

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
        && !ALLOWED_AVATAR_PREFIXES.iter().any(|p| avatar_trimmed.starts_with(p))
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

    let announcements = Announcement::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .take(3)
        .collect();

    let avail_response_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(DISTINCT user_id) FROM availability WHERE reunion_id = $1",
    )
    .bind(reunion_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(0);

    let member_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM users WHERE deactivated_at IS NULL AND email_verified_at IS NOT NULL",
    )
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
    .unwrap_or_default();
    let ra_names = ra_name_list.join(", ");

    Ok(ReunionOverviewPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, ""),
        tab_label: "Overview",
        reunion,
        reunion_date,
        is_ra,
        announcements,
        avail_response_count,
        member_count,
        location_count,
        base_url,
        ra_names,
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
        .await
        .unwrap_or_default();
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

    // Heatmap (RA only)
    let heatmap = if is_ra {
        let total = Availability::respondent_count(state.db(), reunion_id)
            .await
            .unwrap_or(1)
            .max(1);
        Availability::heatmap(state.db(), reunion_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                let pct = (e.member_count as f64 / total as f64 * 100.0).min(100.0);
                HeatmapRow {
                    date: e.available_date.format("%a, %b %d").to_string(),
                    count: e.member_count,
                    total,
                    pct,
                }
            })
            .collect()
    } else {
        vec![]
    };

    Ok(AvailabilityPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "availability"),
        tab_label: "Availability",
        reunion,
        reunion_date,
        my_dates_json,
        months,
        editable,
        is_ra,
        heatmap,
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
        .await
        .unwrap_or_default();

    let mut locations = Vec::new();
    for c in candidates {
        let (avg_score, vote_count, my_vote) =
            LocationVote::aggregate_for_candidate(state.db(), c.id, user.id).await.unwrap_or((None, 0, None));
        let avg_score_str = if votes_revealed {
            avg_score.map(|v| format!("{:.1}", v)).unwrap_or_default()
        } else {
            String::new()
        };
        let selected = reunion.selected_location_id == Some(c.id);
        let my_vote_score = my_vote.as_ref().map(|v| v.score);
        let my_vote_comment = my_vote.and_then(|v| v.comment);
        let ra_votes = if is_ra {
            LocationVote::votes_with_names_for_candidate(state.db(), c.id)
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
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
        tabs: reunion_tabs(reunion_id, "locations"),
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
    let blocks = ScheduleBlock::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();

    // User's signup slot IDs — fetched once, used to annotate each slot
    let user_signup_slot_ids: std::collections::HashSet<Uuid> =
        Signup::list_for_user_in_reunion(state.db(), reunion_id, user.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.signup_slot_id)
            .collect();

    // Build page view blocks with user_signed_up per slot
    let mut days: Vec<ScheduleDay> = Vec::new();
    for block in blocks {
        let slots_raw = SignupSlot::list_for_block(state.db(), block.id)
            .await
            .unwrap_or_default();
        let mut slot_views = Vec::new();
        for slot in slots_raw {
            let signups = Signup::list_for_slot(state.db(), slot.id)
                .await
                .unwrap_or_default();
            let signup_count = signups.len() as i32;
            let is_full = slot.max_count.map(|m| signup_count >= m).unwrap_or(false);
            let user_signed_up = user_signup_slot_ids.contains(&slot.id);
            slot_views.push(ScheduleSlotPageView { slot, signups, is_full, user_signed_up });
        }
        let label = block.block_date.format("%A, %B %-d").to_string();
        let block_view = ScheduleBlockPageView { block, slots: slot_views };
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
        tabs: reunion_tabs(reunion_id, "schedule"),
        tab_label: "Schedule",
        reunion,
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
        tabs: reunion_tabs(reunion_id, "today"),
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
    use crate::models::activity::ActivityVote;

    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;
    let reunion_date = ReunionDate::find_for_reunion(state.db(), reunion_id)
        .await
        .ok()
        .flatten();

    let ideas = ActivityIdea::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();

    let mut activities = Vec::new();
    for idea in ideas {
        let summary = ActivityIdea::for_idea(state.db(), idea.id)
            .await
            .unwrap_or(ActivitySummary {
                idea_id: idea.id,
                avg_interest: None,
                vote_count: 0,
                comment_count: 0,
            });
        let my_vote = ActivityVote::by_user(state.db(), idea.id, user.id)
            .await
            .ok()
            .flatten()
            .map(|v| v.interest_score);
        let avg_interest_str = summary
            .avg_interest
            .map(|v| format!("{:.1}", v))
            .unwrap_or_default();
        let rsvp_rows = sqlx::query_as::<_, (uuid::Uuid, String)>(
            "SELECT ar.user_id, u.display_name
             FROM activity_rsvps ar
             JOIN users u ON u.id = ar.user_id
             WHERE ar.activity_idea_id = $1
             ORDER BY u.display_name",
        )
        .bind(idea.id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default();
        let rsvp_count = rsvp_rows.len() as i64;
        let my_rsvp = rsvp_rows.iter().any(|(uid, _)| *uid == user.id);
        let rsvp_names_str = rsvp_rows.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ");
        let proposer: (String, Option<String>) = sqlx::query_as(
            "SELECT u.display_name, f.name
             FROM users u
             LEFT JOIN family_units f ON f.id = u.family_unit_id
             WHERE u.id = $1",
        )
        .bind(idea.proposed_by)
        .fetch_optional(state.db())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| ("Unknown".to_string(), None));
        let is_own_idea = idea.proposed_by == user.id;
        activities.push(ActivityPageView {
            idea,
            avg_interest_str,
            vote_count: summary.vote_count,
            comment_count: summary.comment_count,
            my_vote,
            rsvp_count,
            my_rsvp,
            rsvp_names_str,
            proposed_by_name: proposer.0,
            proposed_by_family: proposer.1,
            is_own_idea,
        });
    }

    let default_activity_minutes = reunion.default_activity_duration_minutes;
    Ok(ActivitiesPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "activities"),
        tab_label: "Activities",
        reunion,
        reunion_date,
        activities,
        is_ra,
        default_activity_minutes,
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
        .await
        .unwrap_or_default();

    Ok(MediaPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "media"),
        tab_label: "Photos",
        reunion,
        media,
        can_delete_media: is_ra || user.is_sysadmin(),
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
        .await
        .unwrap_or_default();
    let all_users = User::list_all(state.db()).await.unwrap_or_default();

    let expenses = expense_list
        .into_iter()
        .map(|e| {
            let paid_by_name = all_users
                .iter()
                .find(|u| u.id == e.paid_by_user_id)
                .map(|u| u.display_name.clone())
                .unwrap_or_else(|| e.paid_by_user_id.to_string());
            let dollars = e.amount_cents / 100;
            let cents = (e.amount_cents % 100).abs();
            let amount_str = format!("${}.{:02}", dollars, cents);
            ExpensePageView { expense: e, paid_by_name, amount_str }
        })
        .collect();

    let balance_data = Expense::balances_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();

    let balances = balance_data
        .into_iter()
        .map(|b| {
            let user_name = all_users
                .iter()
                .find(|u| u.id == b.user_id)
                .map(|u| u.display_name.clone())
                .unwrap_or_else(|| b.user_id.to_string());
            let dollars = b.net_cents / 100;
            let cents = (b.net_cents % 100).unsigned_abs();
            let net_dollars = format!("{}.{:02}", dollars.abs(), cents);
            BalanceView { user_name, net_cents: b.net_cents, net_dollars }
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
        tabs: reunion_tabs(reunion_id, "expenses"),
        tab_label: "Expenses",
        reunion,
        expenses,
        balances,
        members: all_users,
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
        .await
        .unwrap_or_default();
    // Current user's own responses (may be multiple per question)
    let own_responses = SurveyResponse::list_for_user(state.db(), reunion_id, user.id)
        .await
        .unwrap_or_default();
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
        tabs: reunion_tabs(reunion_id, "survey"),
        tab_label: "Survey",
        reunion,
        questions,
        is_ra,
    }
    .into_response())
}

// ── GET /reunions/:id/announcements ───────────────────────────────────────────

pub async fn announcements_page(
    session: Session,
    State(state): State<AppState>,
    SlugOrId(reunion_id): SlugOrId,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = helpers::load_reunion_for_member(&state, &user, reunion_id).await?;
    let flash = take_flash(&session).await;
    let is_ra = helpers::user_is_ra(&state, &user, reunion_id).await;

    let announcements = Announcement::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();

    Ok(AnnouncementsPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "announcements"),
        tab_label: "Announcements",
        reunion,
        announcements,
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
    let reunion_date = ReunionDate::find_for_reunion(state.db(), reunion_id).await.ok().flatten();
    let raw_family_units = FamilyUnit::list_all(state.db()).await.unwrap_or_default();
    let enrolled_ids = ReunionFamilyUnit::list_ids_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();
    let family_units: Vec<FamilyUnitWithEnrolled> = raw_family_units
        .into_iter()
        .map(|fu| {
            let enrolled = enrolled_ids.contains(&fu.id);
            FamilyUnitWithEnrolled { id: fu.id, name: fu.name, enrolled }
        })
        .collect();
    let ra_ids = ReunionAdmin::list_ids_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();
    let all_users_raw = User::list_all(state.db()).await.unwrap_or_default();
    let all_users_with_ra: Vec<UserWithRaStatus> = all_users_raw
        .into_iter()
        .map(|u| {
            let is_ra = ra_ids.contains(&u.id);
            UserWithRaStatus { id: u.id, display_name: u.display_name, email: u.email, is_ra }
        })
        .collect();

    Ok(SettingsPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        flash,
        tabs: reunion_tabs(reunion_id, "settings"),
        tab_label: "Settings",
        reunion,
        reunion_date,
        family_units,
        all_users_with_ra,
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

    let users = User::list_all(state.db()).await.unwrap_or_default();
    let family_units = FamilyUnit::list_all(state.db()).await.unwrap_or_default();

    let (total_bytes, total_files): (Option<i64>, i64) = sqlx::query_as(
        "SELECT SUM(file_size_bytes), COUNT(*) FROM media",
    )
    .fetch_one(state.db())
    .await
    .unwrap_or((Some(0), 0));

    let total_mb = format!("{:.1}", total_bytes.unwrap_or(0) as f64 / 1_048_576.0);
    let storage = StorageStatsView { total_files, total_mb };

    let all_reunions = Reunion::list_all(state.db()).await.unwrap_or_default();

    // Load all reunion_admins in one query
    let all_admin_rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT ra.reunion_id, ra.user_id, u.display_name
         FROM reunion_admins ra JOIN users u ON u.id = ra.user_id"
    )
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

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
