# Familyer — Design Specification

Self-hosted family reunion facilitating app. Single binary, Docker Compose deployment,
Postgres-backed. Members plan, vote, schedule, and share media for recurring family reunions.

---

## Status Key

| Symbol | Meaning |
|--------|---------|
| `[done]` | Implemented and tested |
| `[planned]` | Decided on, not yet started |
| `[rejected]` | Considered and decided against |
| `[open]` | Undecided; needs discussion |

---

## Table of Contents

1. [Roles](#roles)
2. [Technical Stack](#technical-stack)
3. [Phase Machine](#phase-machine)
4. [Feature Modules](#feature-modules)
5. [Data Model](#data-model)
6. [API Surface](#api-surface)
7. [Deployment](#deployment)
8. [Open Questions](#open-questions)

---

## Roles

| Role | Description | Status |
|------|-------------|--------|
| **Sysadmin** | Technical owner. Manages users, storage, app config, emergency phase overrides. There may be multiple sysadmins. | `[done]` |
| **Responsible Admin (RA)** | Designated planner for a specific reunion — "it's your year." Controls phase progression, schedule, and location candidates for their reunion. | `[done]` |
| **Member** | Authenticated family member. Participates in all phases. | `[done]` |

Sysadmins can always act as RA for any reunion. RAs can only act as RA for their assigned reunion.

---

## Technical Stack

| Concern | Choice | Status | Notes |
|---------|--------|--------|-------|
| Web framework | Axum 0.7 | `[done]` | |
| Async runtime | Tokio | `[done]` | |
| Database | PostgreSQL 16 | `[done]` | |
| ORM / query layer | SQLx 0.8 (async, no macros yet) | `[done]` | Switch to `query!` macros after DB is live; run `cargo sqlx prepare` to commit `.sqlx/` |
| Migrations | sqlx-cli (`sqlx migrate`) | `[done]` | Auto-applied at startup |
| Sessions | tower-sessions 0.12 + Postgres store | `[done]` | Middleware wired in `main.rs`; store auto-migrates at startup |
| Auth — email/password | argon2 | `[done]` | |
| Auth — Google OAuth2 | oauth2 crate v4 | `[done]` | Scopes: `openid email profile`. No app review needed. |
| Auth — Apple / Facebook | — | `[rejected]` | Requires paid Apple dev account or Meta policy overhead; family app doesn't need it |
| Auth — GitHub | — | `[rejected]` | No reason for family members to have GitHub accounts |
| Email | lettre 0.11 (SMTP) | `[done]` | Dev: Mailpit; prod: configurable relay |
| Templating | Askama 0.12 (compile-time) | `[done]` | Templates in `templates/`; all pages wired in `src/routes/pages.rs` |
| Static assets | rust-embed 8 (embedded in binary) | `[done]` | `assets/app.css` + `assets/app.js` served at `/assets/*` |
| Real-time today view | Axum SSE (`axum::response::Sse`) | `[done]` | 30 s interval; fires immediately on connect |
| Calendar export | Manual ICS generation | `[done]` | `GET /reunions/:id/schedule.ics` |
| Bulk zip download | zip crate 2 | `[done]` | On-the-fly in memory; acceptable for typical family media volumes |
| Config | dotenvy + env vars | `[done]` | See `.env.example` |
| Containerization | Docker multi-stage build | `[done]` | Single binary image |
| Compose stack | app + postgres + mailpit | `[done]` | `docker-compose.yml` |

---

## Phase Machine

Reunions move through phases in strict sequential order.
Only the RA (or sysadmin) can advance a phase. Members can act only within the current phase.

**Exception:** Activity ideas are not phase-gated — any member may post or vote on ideas at any time from `draft` onward.

```
draft
  └─> availability          Members mark available days; RA sees heatmap
        └─> date_selected   RA picks date range from heatmap
              └─> locations  RA adds location candidates; members vote (blind)
                    └─> location_selected  RA reveals votes and picks winner
                          └─> schedule     RA builds daily schedule blocks
                                └─> active         Live reunion; full access
                                      └─> post_reunion  Survey open; read-mostly
                                            └─> archived   Permanent read-only
```

### Phase Transition Rules

| From | To | Preconditions (enforced) | Status |
|------|----|--------------------------|--------|
| `draft` | `availability` | None | `[done]` |
| `availability` | `date_selected` | RA must supply a date range | `[done]` |
| `date_selected` | `locations` | None | `[done]` |
| `locations` | `location_selected` | RA must pick a winning location | `[done]` |
| `location_selected` | `schedule` | None | `[done]` |
| `schedule` | `active` | None | `[done]` |
| `active` | `post_reunion` | None; seeds default survey questions | `[done]` |
| `post_reunion` | `archived` | None | `[done]` |

Concurrent advance attempts are blocked at the DB level: `advance_phase()` uses `WHERE phase = $current` so a second concurrent call returns a Conflict error rather than silently double-advancing.

---

## Feature Modules

### Auth & Accounts

| Feature | Status | Notes |
|---------|--------|-------|
| Email/password registration with email verification | `[done]` | `POST /auth/register`; token in `email_verifications`; 24h expiry |
| Password reset via email token | `[done]` | `POST /auth/forgot-password` + `POST /auth/reset-password`; 1h expiry; prior tokens invalidated |
| Google OAuth2 login | `[done]` | PKCE flow; links to existing account on email match |
| Session management (login / logout) | `[done]` | tower-sessions + Postgres store |
| Email verification gate | `[done]` | Google accounts pre-verified |
| Password hashing (argon2id, async) | `[done]` | Runs in `spawn_blocking` |
| Session extractors | `[done]` | `CurrentUser`, `OptionalUser`, `RequireSysadmin` |
| Email transport (lettre SMTP) | `[done]` | Verification + reset + phase-notification helpers |
| Google OAuth2 client | `[done]` | Optional — disabled when credentials absent |
| Token generation | `[done]` | 64-char alphanumeric CSPRNG |
| Profile update (display name, avatar) | `[done]` | `PATCH /me` |
| User model + DB queries | `[done]` | `src/models/user.rs` |
| FamilyUnit model + DB queries | `[done]` | `src/models/user.rs` |

### Reunion Lifecycle

| Feature | Status | Notes |
|---------|--------|-------|
| Create / edit reunion (title, description, assign RA) | `[done]` | |
| Phase machine — core logic + unit tests | `[done]` | `src/phase.rs` — 7 tests |
| Phase machine — DB advance with concurrency guard | `[done]` | Optimistic concurrency via `WHERE phase = $current` |
| Phase advance route (RA/sysadmin only) | `[done]` | Validates preconditions per phase |
| Post-transition side effects (survey seed on active→post_reunion) | `[done]` | |
| Set reunion dates | `[done]` | |
| Reunion model + DB queries | `[done]` | `src/models/reunion.rs` |

### Availability (Phase: `availability`)

| Feature | Status | Notes |
|---------|--------|-------|
| Member marks/unmarks available days | `[done]` | Full atomic replace on `PUT` |
| Heatmap aggregate (date → member count) | `[done]` | |
| RA views heatmap and picks date range | `[done]` | |
| Availability model + DB queries | `[done]` | `src/models/availability.rs` |

### Location Selection (Phases: `locations`, `location_selected`)

| Feature | Status | Notes |
|---------|--------|-------|
| RA adds location candidates (title, description, external URL, capacity, cost, image) | `[done]` | |
| Member votes on each location using 1–5 scale with optional comment | `[done]` | |
| Votes blind by default — individual votes hidden until RA reveals | `[done]` | |
| RA views live aggregate scores at all times | `[done]` | |
| RA reveals votes (all members see full breakdown) | `[done]` | |
| RA picks winner (may override vote result) | `[done]` | |
| Location candidate / vote / summary models + DB queries | `[done]` | `src/models/location.rs` |

### Schedule (Phases: `schedule`, `active`)

| Feature | Status | Notes |
|---------|--------|-------|
| RA builds day-by-day schedule blocks | `[done]` | |
| Block types: `group` (expected), `optional`, `meal`, `travel` with color coding | `[done]` | |
| Implicit free time — unscheduled time needs no entry | `[done]` | Design decision |
| RA creates signup slots on blocks (role, min/max count) | `[done]` | |
| Member claims / releases signup slot | `[done]` | Row-lock enforces max capacity |
| RA can assign / remove members from slots | `[done]` | `POST/DELETE …/slots/:slot_id/assign[/:user_id]`; bypasses phase + capacity |
| Slot roster visible to all logged-in members | `[done]` | |
| Schedule / signup / slot models + DB queries | `[done]` | `src/models/schedule.rs` |

### Activity Ideas (always open, any phase from `draft`)

| Feature | Status | Notes |
|---------|--------|-------|
| Any member posts an activity idea | `[done]` | No phase gate |
| Members vote interest on ideas (1–5 scale) | `[done]` | |
| Members comment on ideas | `[done]` | |
| RA pins / cancels ideas | `[done]` | |
| RA promotes idea → schedule block | `[done]` | |
| Aggregate interest scores + comment counts in list response | `[done]` | |
| Activity idea / vote / comment models + DB queries | `[done]` | `src/models/activity.rs` |

### "Today" View (Phase: `active`)

| Feature | Status | Notes |
|---------|--------|-------|
| Today's schedule blocks with signup rosters via SSE | `[done]` | `GET /reunions/:id/today`; 30 s interval stream; fires immediately on connect |
| Current block / next block highlighted | `[planned]` | Client-side from the SSE snapshot |
| Countdown to next group activity | `[planned]` | Client-side JS |

### Calendar Export

| Feature | Status | Notes |
|---------|--------|-------|
| `.ics` download of reunion schedule | `[done]` | `GET /reunions/:id/schedule.ics`; standard iCalendar format; works with Google Calendar, Outlook, Apple Calendar |

### Media

| Feature | Status | Notes |
|---------|--------|-------|
| Upload photos / videos to mounted local volume | `[done]` | Multipart; UUID-named files prevent path traversal |
| Allowed types: JPEG, PNG, GIF, WebP, MP4, MOV | `[done]` | `ALLOWED_MIME_TYPES` validation |
| Any logged-in member can list and download | `[done]` | |
| Bulk download as zip (on-the-fly) | `[done]` | `GET /reunions/:id/media/download-all` |
| Uploader can delete their own media | `[done]` | |
| RA / sysadmin can delete any media | `[done]` | |
| No moderation queue | `[rejected]` | Family app; trust members |
| Media model + DB queries | `[done]` | `src/models/media.rs` |

### Shared Expenses

| Feature | Status | Notes |
|---------|--------|-------|
| Any member logs an expense (description, amount, who paid, date, split-among list) | `[done]` | `POST /reunions/:id/expenses` |
| Even-split calculation — no cents lost | `[done]` | `calculate_even_split()` — 5 unit tests |
| Per-member running balance (who owes whom) | `[done]` | `GET /reunions/:id/expenses/balances` |
| RA deletes any expense entry | `[done]` | |
| CSV export of balances | `[done]` | `GET /reunions/:id/expenses/balances.csv` |
| Expense / split models + DB queries | `[done]` | `src/models/expense.rs` |

### Feedback & Survey

| Feature | Status | Notes |
|---------|--------|-------|
| Live freeform feedback (Phase: `active` or `post_reunion`) | `[done]` | `POST /reunions/:id/feedback`; RA-only list |
| Post-reunion survey auto-opens when RA advances to `post_reunion` | `[done]` | Seeds 4 default questions via `seed_defaults()` |
| RA adds / removes custom survey questions | `[done]` | |
| Members submit responses (one per question; editable) | `[done]` | Upsert; `PUT /reunions/:id/survey/questions/:q_id/response` |
| Responses visible to RA + sysadmin only | `[done]` | `GET /reunions/:id/survey/responses` returns grouped view |
| Feedback / survey models + DB queries | `[done]` | `src/models/feedback.rs` |

### Next Host Tracker

| Feature | Status | Notes |
|---------|--------|-------|
| Sysadmin assigns "next host" family unit | `[done]` | `POST /admin/host-rotation/:id/set-next`; partial unique index enforces single next |
| History of past host assignments (family unit ↔ reunion) | `[done]` | `GET /admin/host-rotation` |
| `HostRotation` model + DB queries | `[done]` | `src/models/host_rotation.rs` |

### Announcements & Notifications

| Feature | Status | Notes |
|---------|--------|-------|
| RA posts announcements to all members | `[done]` | `POST /reunions/:id/announcements` |
| In-app notification bell (unread list + mark-read) | `[done]` | `GET /me/notifications`, `POST /me/notifications/read-all` |
| Email notifications on phase transitions / announcements | `[done]` | Fired as background `tokio::spawn` after `advance_phase` and `create_announcement`; non-fatal |
| Announcement + notification models + DB queries | `[done]` | `src/models/announcement.rs` |

### Sysadmin Panel

| Feature | Status | Notes |
|---------|--------|-------|
| User list | `[done]` | `GET /admin/users` |
| Role change / deactivate user | `[done]` | `PATCH /admin/users/:id` |
| Family unit management (create, rename) | `[done]` | `GET/POST /admin/family-units`, `PATCH /admin/family-units/:id` |
| Storage usage stats (total bytes, total files) | `[done]` | `GET /admin/storage` |
| Host rotation management | `[done]` | `GET/POST /admin/host-rotation`, set-next, delete |
| Emergency phase override | `[done]` | `POST /admin/reunions/:id/set-phase` — no preconditions |
| App config viewer (non-secret fields only) | `[done]` | `GET /admin/config` — omits DATABASE_URL, SESSION_SECRET, SMTP credentials, OAuth keys |

---

## Data Model

All tables use `UUID` primary keys (`gen_random_uuid()`). All timestamps are `TIMESTAMPTZ`.
Migration: `migrations/001_initial.sql`.

```
family_units
  └─ users (family_unit_id →)
       ├─ email_verifications
       └─ password_resets

reunions (responsible_admin_id → users, selected_location_id → location_candidates)
  ├─ reunion_dates
  ├─ host_rotation (family_unit_id →)
  ├─ availability (user_id →)
  ├─ location_candidates (added_by → users)
  │    └─ location_votes (user_id →)
  ├─ schedule_blocks (created_by → users)
  │    └─ signup_slots
  │         └─ signups (user_id →)
  ├─ activity_ideas (proposed_by → users, promoted_to_block_id → schedule_blocks)
  │    ├─ activity_votes (user_id →)
  │    └─ activity_comments (user_id →)
  ├─ media (uploaded_by → users)
  ├─ expenses (logged_by, paid_by_user_id → users)
  │    └─ expense_splits (user_id →)
  ├─ announcements (posted_by → users)
  ├─ feedback (user_id →)
  ├─ survey_questions
  │    └─ survey_responses (user_id →)
  └─ (notifications are per-user, not per-reunion)

notifications (user_id →)
```

### PostgreSQL Enums

| Type | Values |
|------|--------|
| `reunion_phase` | `draft`, `availability`, `date_selected`, `locations`, `location_selected`, `schedule`, `active`, `post_reunion`, `archived` |
| `user_role` | `sysadmin`, `member` |
| `block_type` | `group`, `optional`, `meal`, `travel` |
| `activity_status` | `proposed`, `pinned`, `scheduled`, `cancelled` |

---

## API Surface

All routes are under the `AppState` extractor. Auth is enforced via session middleware.
JSON in, JSON out (plus SSE for today-view, multipart for uploads, `.ics` / `.csv` / `.zip` downloads).

### Auth routes (`/auth`)

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `POST` | `/auth/register` | Public | `[done]` |
| `POST` | `/auth/login` | Public | `[done]` |
| `POST` | `/auth/logout` | Logged in | `[done]` |
| `GET` | `/auth/verify-email` | Public (token param) | `[done]` |
| `POST` | `/auth/forgot-password` | Public | `[done]` |
| `POST` | `/auth/reset-password` | Public (token param) | `[done]` |
| `GET` | `/auth/google` | Public | `[done]` |
| `GET` | `/auth/google/callback` | Public | `[done]` |

### Profile routes (`/me`)

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/me` | Member | `[done]` |
| `PATCH` | `/me` | Member | `[done]` |
| `GET` | `/me/notifications` | Member | `[done]` |
| `POST` | `/me/notifications/read-all` | Member | `[done]` |
| `POST` | `/me/notifications/:notif_id/read` | Member | `[done]` |

### Reunion routes (`/reunions`)

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions` | Member | `[done]` |
| `POST` | `/reunions` | Sysadmin | `[done]` |
| `GET` | `/reunions/:id` | Member | `[done]` |
| `PATCH` | `/reunions/:id` | RA / Sysadmin | `[done]` |
| `POST` | `/reunions/:id/advance-phase` | RA / Sysadmin | `[done]` |
| `POST` | `/reunions/:id/dates` | RA / Sysadmin | `[done]` |

### Availability routes

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions/:id/availability/me` | Member | `[done]` |
| `PUT` | `/reunions/:id/availability/me` | Member (phase: availability) | `[done]` |
| `GET` | `/reunions/:id/availability/heatmap` | RA / Sysadmin | `[done]` |

### Location routes

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions/:id/locations` | Member | `[done]` |
| `POST` | `/reunions/:id/locations` | RA / Sysadmin | `[done]` |
| `DELETE` | `/reunions/:id/locations/:loc_id` | RA / Sysadmin | `[done]` |
| `PUT` | `/reunions/:id/locations/:loc_id/vote` | Member (phase: locations) | `[done]` |
| `POST` | `/reunions/:id/locations/reveal` | RA / Sysadmin | `[done]` |
| `POST` | `/reunions/:id/locations/:loc_id/select` | RA / Sysadmin | `[done]` |

### Schedule routes

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions/:id/schedule` | Member | `[done]` |
| `POST` | `/reunions/:id/schedule` | RA / Sysadmin | `[done]` |
| `PATCH` | `/reunions/:id/schedule/:block_id` | RA / Sysadmin | `[done]` |
| `DELETE` | `/reunions/:id/schedule/:block_id` | RA / Sysadmin | `[done]` |
| `POST` | `/reunions/:id/schedule/:block_id/slots` | RA / Sysadmin | `[done]` |
| `POST` | `/reunions/:id/schedule/:block_id/slots/:slot_id/claim` | Member | `[done]` |
| `DELETE` | `/reunions/:id/schedule/:block_id/slots/:slot_id/claim` | Member | `[done]` |
| `POST` | `/reunions/:id/schedule/:block_id/slots/:slot_id/assign` | RA / Sysadmin | `[done]` |
| `DELETE` | `/reunions/:id/schedule/:block_id/slots/:slot_id/assign/:user_id` | RA / Sysadmin | `[done]` |
| `GET` | `/reunions/:id/today` | Member (SSE) | `[done]` |
| `GET` | `/reunions/:id/schedule.ics` | Member | `[done]` |

### Activity routes

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions/:id/activities` | Member | `[done]` |
| `POST` | `/reunions/:id/activities` | Member | `[done]` |
| `PUT` | `/reunions/:id/activities/:act_id/vote` | Member | `[done]` |
| `GET` | `/reunions/:id/activities/:act_id/comments` | Member | `[done]` |
| `POST` | `/reunions/:id/activities/:act_id/comments` | Member | `[done]` |
| `DELETE` | `/reunions/:id/activities/:act_id/comments/:cmt_id` | Owner / RA / Sysadmin | `[done]` |
| `PATCH` | `/reunions/:id/activities/:act_id/status` | RA / Sysadmin | `[done]` |
| `POST` | `/reunions/:id/activities/:act_id/promote` | RA / Sysadmin | `[done]` |

### Media routes

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions/:id/media` | Member | `[done]` |
| `POST` | `/reunions/:id/media` | Member (multipart) | `[done]` |
| `GET` | `/reunions/:id/media/:media_id` | Member (download) | `[done]` |
| `DELETE` | `/reunions/:id/media/:media_id` | Uploader / RA / Sysadmin | `[done]` |
| `GET` | `/reunions/:id/media/download-all` | Member (zip) | `[done]` |

### Expense routes

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions/:id/expenses` | Member | `[done]` |
| `POST` | `/reunions/:id/expenses` | Member | `[done]` |
| `DELETE` | `/reunions/:id/expenses/:exp_id` | RA / Sysadmin | `[done]` |
| `GET` | `/reunions/:id/expenses/balances` | Member | `[done]` |
| `GET` | `/reunions/:id/expenses/balances.csv` | Member | `[done]` |

### Feedback & Survey routes

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions/:id/feedback` | RA / Sysadmin | `[done]` |
| `POST` | `/reunions/:id/feedback` | Member (phase: active+) | `[done]` |
| `GET` | `/reunions/:id/survey/questions` | Member | `[done]` |
| `POST` | `/reunions/:id/survey/questions` | RA / Sysadmin | `[done]` |
| `DELETE` | `/reunions/:id/survey/questions/:q_id` | RA / Sysadmin | `[done]` |
| `PUT` | `/reunions/:id/survey/questions/:q_id/response` | Member (phase: post_reunion+) | `[done]` |
| `GET` | `/reunions/:id/survey/responses` | RA / Sysadmin | `[done]` |

### Announcement routes

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/reunions/:id/announcements` | Member | `[done]` |
| `POST` | `/reunions/:id/announcements` | RA / Sysadmin | `[done]` |
| `DELETE` | `/reunions/:id/announcements/:ann_id` | RA / Sysadmin | `[done]` |

### Sysadmin routes (`/admin`)

| Method | Path | Access | Status |
|--------|------|--------|--------|
| `GET` | `/admin/users` | Sysadmin | `[done]` |
| `PATCH` | `/admin/users/:id` | Sysadmin | `[done]` |
| `GET` | `/admin/family-units` | Sysadmin | `[done]` |
| `POST` | `/admin/family-units` | Sysadmin | `[done]` |
| `PATCH` | `/admin/family-units/:id` | Sysadmin | `[done]` |
| `GET` | `/admin/host-rotation` | Sysadmin | `[done]` |
| `POST` | `/admin/host-rotation` | Sysadmin | `[done]` |
| `POST` | `/admin/host-rotation/:id/set-next` | Sysadmin | `[done]` |
| `DELETE` | `/admin/host-rotation/:id` | Sysadmin | `[done]` |
| `GET` | `/admin/storage` | Sysadmin | `[done]` |
| `GET` | `/admin/config` | Sysadmin | `[done]` |
| `POST` | `/admin/reunions/:id/set-phase` | Sysadmin | `[done]` |

---

## Deployment

### Docker Compose Stack

| Service | Image | Purpose | Status |
|---------|-------|---------|--------|
| `app` | Multi-stage Rust build | Application binary | `[done]` |
| `db` | postgres:16-alpine | Primary data store | `[done]` |
| `mailpit` | axllent/mailpit | Dev SMTP catcher + web UI | `[done]` |

### Configuration (`.env`)

All configuration via environment variables. See `.env.example` for full reference.

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `DATABASE_URL` | Yes | — | Postgres connection string |
| `SESSION_SECRET` | Yes | — | HMAC session signing key (generate with `openssl rand -hex 64`) |
| `GOOGLE_CLIENT_ID` | No | — | Leave empty to disable Google login |
| `GOOGLE_CLIENT_SECRET` | No | — | |
| `GOOGLE_REDIRECT_URL` | No | `…/auth/google/callback` | |
| `APP_BASE_URL` | No | `http://localhost:8080` | Used in email links |
| `APP_PORT` | No | `8080` | |
| `SMTP_HOST` | No | `localhost` | |
| `SMTP_PORT` | No | `1025` | |
| `SMTP_FROM` | No | `familyer@localhost` | |
| `SMTP_TLS` | No | `false` | |
| `MEDIA_STORAGE_PATH` | No | `./media` | Host path; mapped to `/data/media` in container |
| `MAX_UPLOAD_BYTES` | No | `104857600` (100 MB) | Per-file upload limit |

### Build Notes

- The Dockerfile sets `SQLX_OFFLINE=true` for hermetic builds.
- Before building the Docker image, run `cargo sqlx prepare` locally (with a running DB) to generate `.sqlx/`. Commit the `.sqlx/` directory.
- Migrations run automatically at startup via `sqlx::migrate!`.

---

## Open Questions

| # | Question | Status |
|---|----------|--------|
| 1 | Zip download generates in-memory — acceptable for typical family reunion media volumes (< a few GB), but large reunions may want a pre-generated async job. | `[open]` |
| 2 | Should location image uploads use the same media pipeline, or a separate upload field on the candidate? | `[open]` |
| 3 | Should expense splits always be even, or support arbitrary per-member amounts? Current model supports arbitrary via `expense_splits` table. | `[open]` |
| 4 | Do we want any rate limiting on uploads or vote endpoints? | `[open]` |
| 5 | Should members be able to see who voted for activities (interest votes are not explicitly hidden)? | `[open]` |
| 6 | Email notifications target all active verified users in the system. If reunion membership becomes more granular, the notification recipient list will need updating. | `[open]` |

---

## Rejected Ideas

| Idea | Reason |
|------|--------|
| Apple Sign In | Requires $99/yr Apple Developer membership; quirky JWT flow; name only sent on first login |
| Facebook / Meta Login | Complex developer portal; policy overhead; not appropriate for private family app |
| GitHub OAuth | Family members don't need GitHub accounts |
| Media moderation queue (RA approves before visible) | Over-engineered for a family app; trust members |
| Phase-gating activity ideas | Defeats the purpose — gauging interest is useful at any stage |
| SQLite as default | Went with Postgres for concurrency and production robustness |
| Separate frontend SPA | Single binary with server-rendered templates (Askama) is simpler to self-host |
