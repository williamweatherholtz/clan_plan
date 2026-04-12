// ── HTML page handlers (server-rendered with Askama) ─────────────────────────
//
// These routes live at human-friendly paths (/login, /dashboard, /reunions/:id,
// etc.) and render Askama templates.  The JSON API routes continue to live
// under /api/*.

use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
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
        session::{save_user_id, SESSION_USER_ID},
    },
    error::AppError,
    models::{
        activity::{ActivityIdea, ActivitySummary},
        announcement::{Announcement, Notification},
        availability::Availability,
        expense::Expense,
        feedback::SurveyQuestion,
        host_rotation::HostRotation,
        location::LocationCandidate,
        media::Media,
        reunion::{Reunion, ReunionDate},
        schedule::{ScheduleBlock, Signup},
        user::{FamilyUnit, User},
    },
    phase::Phase,
    state::AppState,
};

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

// ── Reunion tab helper ────────────────────────────────────────────────────────

pub struct NavTab {
    pub path: String,
    pub label: &'static str,
    pub active: bool,
}

fn reunion_tabs(_reunion_id: Uuid, active_path: &str) -> Vec<NavTab> {
    let tabs: &[(&str, &str)] = &[
        ("", "Overview"),
        ("availability", "Availability"),
        ("locations", "Locations"),
        ("schedule", "Schedule"),
        ("today", "Today"),
        ("activities", "Activities"),
        ("media", "Photos"),
        ("expenses", "Expenses"),
        ("survey", "Survey"),
        ("announcements", "Announcements"),
    ];
    tabs.iter()
        .map(|(path, label)| NavTab {
            path: path.to_string(),
            label,
            active: *path == active_path,
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
}

// ── Activity view type ────────────────────────────────────────────────────────

pub struct ActivityPageView {
    pub idea: ActivityIdea,
    pub avg_interest_str: String,
    pub vote_count: i64,
    pub comment_count: i64,
    pub my_vote: Option<i16>,
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

pub struct SurveyQuestionView {
    pub question: SurveyQuestion,
    pub my_response: String,
}

// ── Host rotation view ────────────────────────────────────────────────────────

pub struct HostRotationView {
    pub id: Uuid,
    pub family_unit_name: String,
    pub is_next: bool,
    pub reunion_year: Option<String>,
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

#[derive(Template)]
#[template(path = "pages/dashboard.html")]
struct DashboardPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunions: Vec<Reunion>,
}

#[derive(Template)]
#[template(path = "pages/profile.html")]
struct ProfilePage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    email: String,
    display_name: String,
    avatar_url: String,
}

#[derive(Template)]
#[template(path = "pages/reunion.html")]
struct ReunionOverviewPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    reunion_date: Option<ReunionDate>,
    is_ra: bool,
    tabs: Vec<NavTab>,
    announcements: Vec<Announcement>,
}

#[derive(Template)]
#[template(path = "pages/availability.html")]
struct AvailabilityPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    my_dates_json: String,
    months: Vec<CalendarMonth>,
    editable: bool,
    is_ra: bool,
    heatmap: Vec<HeatmapRow>,
}

#[derive(Template)]
#[template(path = "pages/locations.html")]
struct LocationsPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    locations: Vec<LocationPageView>,
    votes_revealed: bool,
    can_vote: bool,
    is_ra: bool,
}

#[derive(Template)]
#[template(path = "pages/schedule.html")]
struct SchedulePage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    days: Vec<ScheduleDay>,
    is_ra: bool,
}

#[derive(Template)]
#[template(path = "pages/today.html")]
struct TodayPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
}

#[derive(Template)]
#[template(path = "pages/activities.html")]
struct ActivitiesPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    activities: Vec<ActivityPageView>,
    is_ra: bool,
}

#[derive(Template)]
#[template(path = "pages/media.html")]
struct MediaPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    media: Vec<Media>,
    can_delete_media: bool,
}

#[derive(Template)]
#[template(path = "pages/expenses.html")]
struct ExpensesPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    expenses: Vec<ExpensePageView>,
    balances: Vec<BalanceView>,
    members: Vec<User>,
    current_user_id: Uuid,
    is_ra: bool,
}

#[derive(Template)]
#[template(path = "pages/survey.html")]
struct SurveyPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    questions: Vec<SurveyQuestionView>,
    can_respond: bool,
    is_ra: bool,
}

#[derive(Template)]
#[template(path = "pages/announcements.html")]
struct AnnouncementsPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    reunion: Reunion,
    announcements: Vec<Announcement>,
    is_ra: bool,
}

#[derive(Template)]
#[template(path = "pages/admin.html")]
struct AdminPage {
    user_name: String,
    is_sysadmin: bool,
    unread_count: i64,
    flash: Option<FlashMsg>,
    users: Vec<User>,
    family_units: Vec<FamilyUnit>,
    host_rotation: Vec<HostRotationView>,
    storage: StorageStatsView,
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
    RegisterPage {
        flash,
        google_enabled: state.config().google_oauth_enabled(),
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

    if form.password.len() < 8 {
        set_flash(&session, "error", "Password must be at least 8 characters").await;
        return Redirect::to("/register").into_response();
    }
    if form.display_name.trim().is_empty() {
        set_flash(&session, "error", "Display name cannot be empty").await;
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
    let flash = take_flash(&session).await;
    let reunions = Reunion::list_all(state.db()).await.unwrap_or_default();
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    Ok(DashboardPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
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
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    Ok(ProfilePage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
        email: user.email.clone(),
        display_name: user.display_name.clone(),
        avatar_url: user.avatar_url.clone().unwrap_or_default(),
    }
    .into_response())
}

// ── POST /profile ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ProfileForm {
    display_name: String,
    avatar_url: String,
}

pub async fn profile_form(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<ProfileForm>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    if !form.display_name.trim().is_empty() {
        let _ = User::update_display_name(state.db(), user.id, form.display_name.trim()).await;
    }
    let avatar = if form.avatar_url.trim().is_empty() { None } else { Some(form.avatar_url.trim()) };
    let _ = User::set_avatar(state.db(), user.id, avatar).await;
    set_flash(&session, "success", "Profile updated.").await;
    Ok(Redirect::to("/profile").into_response())
}

// ── POST /profile/password ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ChangePasswordForm {
    current_password: String,
    new_password: String,
}

pub async fn change_password_form(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<ChangePasswordForm>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;

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

// ── GET /notifications ────────────────────────────────────────────────────────

pub async fn notifications_page(
    session: Session,
    State(state): State<AppState>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    // Mark all as read
    let _ = Notification::mark_all_read(state.db(), user.id).await;
    Ok(Redirect::to("/dashboard").into_response())
}

// ── Reunion helper ────────────────────────────────────────────────────────────

async fn load_reunion_or_redirect(
    state: &AppState,
    reunion_id: Uuid,
) -> Result<Reunion, Response> {
    Reunion::find_by_id(state.db(), reunion_id)
        .await
        .map_err(|_| Redirect::to("/dashboard").into_response())
}

// ── GET /reunions/:id ─────────────────────────────────────────────────────────

pub async fn reunion_overview(
    session: Session,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);
    let reunion_date = ReunionDate::find_for_reunion(state.db(), reunion_id).await.ok().flatten();
    let announcements = Announcement::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .take(3)
        .collect();
    Ok(ReunionOverviewPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
        tabs: reunion_tabs(reunion_id, ""),
        reunion,
        reunion_date,
        is_ra,
        announcements,
    }
    .into_response())
}

// ── GET /reunions/:id/availability ────────────────────────────────────────────

pub async fn availability_page(
    session: Session,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);

    let my_dates = Availability::for_user(state.db(), reunion_id, user.id)
        .await
        .unwrap_or_default();
    let my_dates_json = serde_json::to_string(
        &my_dates.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());

    // Determine date range to show — use reunion dates or fallback to +3 months
    let (start, end) = {
        let rd = ReunionDate::find_for_reunion(state.db(), reunion_id)
            .await
            .ok()
            .flatten();
        match rd {
            Some(ref d) => (d.start_date, d.end_date),
            None => {
                let today = chrono::Local::now().date_naive();
                (today, today + chrono::Duration::days(90))
            }
        }
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
        unread_count: unread,
        flash,
        reunion,
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
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    use crate::models::location::LocationVote;

    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);
    let votes_revealed = reunion.location_votes_revealed;
    let can_vote = matches!(reunion.phase, Phase::Locations | Phase::LocationSelected);

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
        locations.push(LocationPageView {
            candidate: c,
            avg_score_str,
            vote_count,
            my_vote_score: my_vote,
        });
    }

    Ok(LocationsPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
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
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    use crate::models::schedule::SignupSlot;

    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);

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
        unread_count: unread,
        flash,
        reunion,
        days,
        is_ra,
    }
    .into_response())
}

// ── GET /reunions/:id/today ───────────────────────────────────────────────────

pub async fn today_page(
    session: Session,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    Ok(TodayPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
        reunion,
    }
    .into_response())
}

// ── GET /reunions/:id/activities ──────────────────────────────────────────────

pub async fn activities_page(
    session: Session,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    use crate::models::activity::ActivityVote;

    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);

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
        activities.push(ActivityPageView {
            idea,
            avg_interest_str,
            vote_count: summary.vote_count,
            comment_count: summary.comment_count,
            my_vote,
        });
    }

    Ok(ActivitiesPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
        reunion,
        activities,
        is_ra,
    }
    .into_response())
}

// ── GET /reunions/:id/media ───────────────────────────────────────────────────

pub async fn media_page(
    session: Session,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);

    let media = Media::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();

    Ok(MediaPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
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
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);

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

    Ok(ExpensesPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
        reunion,
        expenses,
        balances,
        members: all_users,
        current_user_id: user.id,
        is_ra,
    }
    .into_response())
}

// ── GET /reunions/:id/survey ──────────────────────────────────────────────────

pub async fn survey_page(
    session: Session,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    use crate::models::feedback::SurveyResponse;

    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);
    let can_respond = matches!(reunion.phase, Phase::PostReunion | Phase::Archived);

    let qs = SurveyQuestion::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();
    let all_responses = SurveyResponse::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();

    let questions = qs
        .into_iter()
        .map(|q| {
            let my_response = all_responses
                .iter()
                .find(|r| r.survey_question_id == q.id && r.user_id == user.id)
                .map(|r| r.response_text.clone())
                .unwrap_or_default();
            SurveyQuestionView { question: q, my_response }
        })
        .collect();

    Ok(SurveyPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
        reunion,
        questions,
        can_respond,
        is_ra,
    }
    .into_response())
}

// ── GET /reunions/:id/announcements ───────────────────────────────────────────

pub async fn announcements_page(
    session: Session,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> Result<Response, Response> {
    let user = require_login(&session, &state).await?;
    let reunion = load_reunion_or_redirect(&state, reunion_id).await?;
    let flash = take_flash(&session).await;
    let unread = Notification::unread_count(state.db(), user.id).await.unwrap_or(0);
    let is_ra = user.is_ra_for(reunion.responsible_admin_id);

    let announcements = Announcement::list_for_reunion(state.db(), reunion_id)
        .await
        .unwrap_or_default();

    Ok(AnnouncementsPage {
        user_name: user.display_name.clone(),
        is_sysadmin: user.is_sysadmin(),
        unread_count: unread,
        flash,
        reunion,
        announcements,
        is_ra,
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
    let unread = Notification::unread_count(state.db(), admin.id).await.unwrap_or(0);

    let users = User::list_all(state.db()).await.unwrap_or_default();
    let family_units = FamilyUnit::list_all(state.db()).await.unwrap_or_default();
    let hr_raw = HostRotation::list_all(state.db()).await.unwrap_or_default();

    let host_rotation = hr_raw
        .into_iter()
        .map(|hr| {
            let family_unit_name = family_units
                .iter()
                .find(|fu| fu.id == hr.family_unit_id)
                .map(|fu| fu.name.clone())
                .unwrap_or_else(|| hr.family_unit_id.to_string());
            HostRotationView {
                id: hr.id,
                family_unit_name,
                is_next: hr.is_next,
                reunion_year: hr.notes.clone(),
            }
        })
        .collect();

    let (total_bytes, total_files): (Option<i64>, i64) = sqlx::query_as(
        "SELECT SUM(file_size_bytes), COUNT(*) FROM media",
    )
    .fetch_one(state.db())
    .await
    .unwrap_or((Some(0), 0));

    let total_mb = format!("{:.1}", total_bytes.unwrap_or(0) as f64 / 1_048_576.0);
    let storage = StorageStatsView { total_files, total_mb };

    Ok(AdminPage {
        user_name: admin.display_name.clone(),
        is_sysadmin: true,
        unread_count: unread,
        flash,
        users,
        family_units,
        host_rotation,
        storage,
    }
    .into_response())
}
