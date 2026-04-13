pub mod activities;
pub mod admin;
pub mod auth;
pub mod availability;
pub mod expenses;
pub mod feedback;
pub mod helpers;
pub mod invites;
pub mod locations;
pub mod media;
pub mod pages;
pub mod reunions;
pub mod schedule;
pub mod today;

use axum::{
    routing::{delete, get, patch, post, put},
    Router,
};

use crate::state::AppState;

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/register",        post(auth::register))
        .route("/login",           post(auth::login))
        .route("/logout",          post(auth::logout))
        .route("/verify-email",    get(auth::verify_email))
        .route("/forgot-password", post(auth::forgot_password))
        .route("/reset-password",  post(auth::reset_password))
        .route("/google",          get(auth::google_start))
        .route("/google/callback", get(auth::google_callback))
}

pub fn me_router() -> Router<AppState> {
    Router::new()
        .route("/me",                              get(auth::get_me).patch(auth::update_me))
}

pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/admin/users",                         get(admin::list_users))
        .route("/admin/users/:id",                     patch(admin::update_user))
        .route("/admin/family-units",                  get(admin::list_family_units)
                                                          .post(admin::create_family_unit))
        .route("/admin/family-units/:id",              patch(admin::rename_family_unit))
        .route("/admin/host-rotation",                 get(admin::list_host_rotation)
                                                          .post(admin::add_host_rotation))
        .route("/admin/host-rotation/:id/set-next",    post(admin::set_next_host))
        .route("/admin/host-rotation/:id",             delete(admin::delete_host_rotation_entry))
        .route("/admin/storage",                       get(admin::storage_stats))
        .route("/admin/config",                        get(admin::get_config))
        .route("/admin/registration",                  patch(admin::update_registration))
        .route("/admin/reunions/:id/set-phase",        post(admin::force_set_phase))
}

pub fn reunions_router() -> Router<AppState> {
    Router::new()
        // ── Reunion CRUD + phase control ─────────────────────────────────────
        .route("/",                        get(reunions::list_reunions).post(reunions::create_reunion))
        .route("/:id",                     get(reunions::get_reunion).patch(reunions::update_reunion)
                                              .delete(reunions::delete_reunion))
        .route("/:id/advance-phase",       post(reunions::advance_phase))
        .route("/:id/retreat-phase",       post(reunions::retreat_phase))
        .route("/:id/dates",               post(reunions::set_dates))
        .route("/:id/my-completion",       get(reunions::my_completion))
        .route("/:id/setup-progress",      get(reunions::setup_progress))
        .route("/:id/family-units",        get(reunions::list_reunion_family_units))
        .route("/:id/family-units/:fu_id", put(reunions::add_reunion_family_unit)
                                              .delete(reunions::remove_reunion_family_unit))
        .route("/:id/admins/:user_id",     put(reunions::add_reunion_admin)
                                              .delete(reunions::remove_reunion_admin))
        .route("/:id/unarchive",           post(reunions::unarchive))
        // ── Availability ──────────────────────────────────────────────────────
        .route("/:id/availability/me",     get(availability::get_my_availability)
                                              .put(availability::set_my_availability))
        .route("/:id/availability/heatmap",get(availability::get_heatmap))
        // ── Locations ─────────────────────────────────────────────────────────
        .route("/:id/locations",           get(locations::list_locations)
                                              .post(locations::create_location))
        .route("/:id/locations/reveal",    post(locations::reveal_votes))
        .route("/:id/locations/:loc_id",   patch(locations::update_location)
                                              .delete(locations::delete_location))
        .route("/:id/locations/:loc_id/vote",   put(locations::vote_location))
        .route("/:id/locations/:loc_id/select", post(locations::select_location))
        // ── Schedule blocks + slots ───────────────────────────────────────────
        .route("/:id/schedule",            get(schedule::get_schedule)
                                              .post(schedule::create_block))
        .route("/:id/schedule/:block_id",  patch(schedule::update_block)
                                              .delete(schedule::delete_block))
        .route("/:id/schedule/:block_id/slots",
                                           post(schedule::create_slot))
        .route("/:id/schedule/:block_id/slots/:slot_id/claim",
                                           post(schedule::claim_slot)
                                              .delete(schedule::release_slot))
        .route("/:id/schedule/:block_id/slots/:slot_id/assign",
                                           post(schedule::admin_assign_slot))
        .route("/:id/schedule/:block_id/slots/:slot_id/assign/:user_id",
                                           delete(schedule::admin_remove_signup))
        // ── Today view (SSE) + calendar export ────────────────────────────────
        .route("/:id/today",               get(today::get_today))
        .route("/:id/schedule.ics",        get(today::get_ics))
        // ── Activity ideas ────────────────────────────────────────────────────
        .route("/:id/activities",          get(activities::list_activities)
                                              .post(activities::create_activity))
        .route("/:id/activities/:act_id",          patch(activities::update_activity)
                                                     .delete(activities::delete_activity))
        .route("/:id/activities/:act_id/vote",    put(activities::vote_activity))
        .route("/:id/activities/:act_id/rsvp",    put(activities::rsvp_activity)
                                                     .delete(activities::unrsvp_activity))
        .route("/:id/activities/:act_id/comments",
                                           get(activities::list_comments)
                                              .post(activities::create_comment))
        .route("/:id/activities/:act_id/comments/:cmt_id",
                                           delete(activities::delete_comment))
        .route("/:id/activities/:act_id/status",  patch(activities::set_status))
        .route("/:id/activities/:act_id/promote", post(activities::promote_activity))
        // ── Invite links ─────────────────────────────────────────────────────
        .route("/:id/invites",                  post(invites::create_invite))
        .route("/:id/invites/:invite_id",       delete(invites::revoke_invite))
        .route("/:id/invite-members/:user_id",  delete(invites::remove_invite_member))
        // ── Media ─────────────────────────────────────────────────────────────
        .route("/:id/media",               get(media::list_media).post(media::upload_media))
        .route("/:id/media/download-all",  get(media::download_all_zip))
        .route("/:id/media/:media_id",     get(media::download_media).delete(media::delete_media))
        // ── Expenses ──────────────────────────────────────────────────────────
        .route("/:id/expenses",            get(expenses::list_expenses).post(expenses::create_expense))
        .route("/:id/expenses/balances",   get(expenses::get_balances))
        .route("/:id/expenses/balances.csv", get(expenses::get_balances_csv))
        .route("/:id/expenses/confirm",    post(expenses::confirm_expenses)
                                              .delete(expenses::unconfirm_expenses))
        .route("/:id/expenses/:exp_id",    delete(expenses::delete_expense))
        // ── Live feedback + survey ────────────────────────────────────────────
        .route("/:id/feedback",            get(feedback::list_feedback).post(feedback::create_feedback))
        .route("/:id/survey/questions",    get(feedback::list_survey_questions)
                                              .post(feedback::create_survey_question))
        .route("/:id/survey/questions/:q_id",
                                           delete(feedback::delete_survey_question))
        .route("/:id/survey/questions/:q_id/responses",
                                           post(feedback::create_survey_response))
        .route("/:id/survey/questions/:q_id/responses/:r_id",
                                           patch(feedback::update_survey_response)
                                          .delete(feedback::delete_survey_response))
        .route("/:id/survey/responses",    get(feedback::list_survey_responses))
}

pub fn pages_router() -> Router<AppState> {
    Router::new()
        // Static assets
        .route("/assets/*path",                     get(pages::serve_asset))
        // Root redirect
        .route("/",                                 get(pages::index))
        // Invite join link
        .route("/join/:token",                      get(pages::join_page))
        // Auth pages (HTML form flows)
        .route("/login",                            get(pages::login_page).post(pages::login_form))
        .route("/register",                         get(pages::register_page).post(pages::register_form))
        .route("/forgot-password",                  get(pages::forgot_password_page).post(pages::forgot_password_form))
        .route("/reset-password",                   get(pages::reset_password_page).post(pages::reset_password_form))
        // App pages
        .route("/dashboard",                        get(pages::dashboard))
        .route("/profile",                          get(pages::profile_page).post(pages::profile_form))
        .route("/profile/password",                 post(pages::change_password_form))
        .route("/admin",                            get(pages::admin_page))
        // Reunion pages
        .route("/reunions/:id",                     get(pages::reunion_overview))
        .route("/reunions/:id/availability",        get(pages::availability_page))
        .route("/reunions/:id/locations",           get(pages::locations_page))
        .route("/reunions/:id/schedule",            get(pages::schedule_page))
        .route("/reunions/:id/today",               get(pages::today_page))
        .route("/reunions/:id/activities",          get(pages::activities_page))
        .route("/reunions/:id/media",               get(pages::media_page))
        .route("/reunions/:id/expenses",            get(pages::expenses_page))
        .route("/reunions/:id/survey",              get(pages::survey_page))
        .route("/reunions/:id/settings",            get(pages::settings_page))
        // Slug-based aliases: /r/:slug/... mirrors /reunions/:id/...
        .route("/r/:slug",                          get(pages::reunion_overview))
        .route("/r/:slug/availability",             get(pages::availability_page))
        .route("/r/:slug/locations",                get(pages::locations_page))
        .route("/r/:slug/schedule",                 get(pages::schedule_page))
        .route("/r/:slug/today",                    get(pages::today_page))
        .route("/r/:slug/activities",               get(pages::activities_page))
        .route("/r/:slug/media",                    get(pages::media_page))
        .route("/r/:slug/expenses",                 get(pages::expenses_page))
        .route("/r/:slug/survey",                   get(pages::survey_page))
        .route("/r/:slug/settings",                 get(pages::settings_page))
}
