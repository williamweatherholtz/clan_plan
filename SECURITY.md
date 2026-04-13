# Security Requirements — Clan Plan

Assessed: 2026-04-12  
Scope: hosted production deployment  
Stack: Rust / Axum 0.7 · PostgreSQL · Askama · Alpine.js · Docker / Podman

---

## Severity Index

| ID | Title | Severity | Status |
|----|-------|----------|--------|
| S-01 | No HTTPS / TLS enforcement | CRITICAL | Open |
| S-02 | Default admin credentials active at first boot | CRITICAL | Open |
| S-03 | No rate limiting on authentication endpoints | HIGH | Open |
| S-04 | No account lockout after repeated failed logins | HIGH | Open |
| S-05 | Missing HTTP security headers | HIGH | Open |
| S-06 | No CSRF protection on state-changing form POSTs | HIGH | Open |
| S-07 | PostgreSQL port exposed to host network | MEDIUM | Open |
| S-08 | Session lifetime 7 days with no absolute expiry | MEDIUM | Open |
| S-09 | WrongPhase error leaks internal state labels | MEDIUM | Open |
| S-10 | No input length limits on free-text fields | MEDIUM | Open |
| S-11 | Mailpit dev container included in compose file | MEDIUM | Open |
| S-12 | User-controlled avatar_url returned in API responses | MEDIUM | Open |
| S-13 | .env not confirmed absent from version control | LOW | Open |
| S-14 | Media path not canonicalized before read/write | LOW | Open |
| S-15 | OAuth CSRF token compared with non-constant-time equality | LOW | Open |

---

## Critical

---

### S-01 — No HTTPS / TLS Enforcement

**Location:** `src/main.rs:113`, `docker-compose.yml:7`

**Finding:**  
The application binds on `0.0.0.0:8080` over plain HTTP. Session cookies, passwords,
OAuth codes, and all user data travel in cleartext between the browser and server.
The session cookie has no `Secure` flag enforced by the application layer.

```rust
// main.rs:113 — binds HTTP only, no TLS
let addr: SocketAddr = format!("0.0.0.0:{}", config.app_port).parse()?;
```

**Requirements:**

1. Terminate TLS at a reverse proxy (Caddy, nginx, or Traefik) placed in front of
   the application container. The app itself does not need to handle TLS directly.

2. Add a Caddy service to `docker-compose.yml`:
   ```yaml
   caddy:
     image: caddy:2-alpine
     ports:
       - "80:80"
       - "443:443"
     volumes:
       - ./Caddyfile:/etc/caddy/Caddyfile
       - caddy_data:/data
     depends_on:
       - app
   ```

   Minimal `Caddyfile`:
   ```
   yourdomain.com {
     reverse_proxy app:8080
   }
   ```
   Caddy handles ACME/Let's Encrypt automatically.

3. Remove the direct port exposure from the `app` service so it is no longer reachable
   without going through the proxy:
   ```yaml
   # app service — remove or restrict the ports mapping
   # ports:
   #   - "${APP_PORT:-8080}:8080"
   ```

4. Add the `Secure` attribute to the session cookie in `src/main.rs`:
   ```rust
   use tower_sessions::cookie::Key;
   
   let session_layer = SessionManagerLayer::new(session_store)
       .with_expiry(Expiry::OnInactivity(Duration::hours(8)))
       .with_same_site(SameSite::Strict)   // see S-06
       .with_secure(true);                 // only send over HTTPS
   ```

---

### S-02 — Default Admin Credentials Active at First Boot

**Location:** `src/main.rs:67–91`, `src/config.rs` (admin_email / admin_password fields)

**Finding:**  
On first startup, if no sysadmin exists in the database, the application creates one
using `ADMIN_EMAIL` / `ADMIN_PASSWORD` from the environment, defaulting to
`admin@localhost` / `password`. The warning is logged but the application continues
running with these credentials indefinitely. No forced password change is implemented.

```rust
// main.rs:78
if config.admin_using_defaults {
    tracing::warn!("... Password: password ...");
}
```

**Requirements:**

1. **Block startup entirely** when defaults are detected in a non-development environment:
   ```rust
   if config.admin_using_defaults {
       let is_dev = std::env::var("APP_ENV").as_deref() == Ok("development");
       if !is_dev {
           anyhow::bail!(
               "ADMIN_EMAIL and ADMIN_PASSWORD must be set in production. \
                Refusing to start with default credentials."
           );
       }
       tracing::warn!("default admin credentials in use — development only");
   }
   ```

2. Add `APP_ENV=production` to the production `.env` and `APP_ENV=development` to
   any local dev `.env`.

3. Set explicit, strong credentials in the production environment:
   ```
   ADMIN_EMAIL=your-real-email@example.com
   ADMIN_PASSWORD=<generated with: openssl rand -base64 32>
   ```

4. After first login, immediately change the admin password through the profile page.
   Document this as a required post-deployment step.

---

## High

---

### S-03 — No Rate Limiting on Authentication Endpoints

**Location:** `src/main.rs:96–111` (router assembly — no rate limit layer)

**Finding:**  
The following endpoints accept unlimited requests per IP:
- `POST /api/auth/login` — password brute force
- `POST /api/auth/forgot-password` — email/reset spam
- `POST /api/auth/register` — account creation spam
- `POST /register` (form equivalent)

An attacker can attempt thousands of password guesses per second with no friction.

**Requirements:**

1. Add `tower_governor` (or `axum-governor`) to `Cargo.toml`:
   ```toml
   tower_governor = "0.4"
   ```

2. Apply per-IP rate limiting to the auth router in `src/main.rs`:
   ```rust
   use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
   
   let auth_governor = GovernorConfigBuilder::default()
       .per_second(2)        // 2 requests per second per IP
       .burst_size(10)       // allow short bursts
       .finish()
       .unwrap();
   
   let api = Router::new()
       .nest("/auth", auth_router().layer(GovernorLayer::new(Arc::new(auth_governor))))
       ...
   ```

3. Ensure the real client IP is visible to the rate limiter when behind a proxy.
   Add `SecureClientIpSource` or configure `axum-client-ip` to read from
   `X-Forwarded-For` (trusted only from the Caddy/nginx container).

---

### S-04 — No Account Lockout After Repeated Failed Logins

**Location:** `src/routes/auth.rs:154–182` (login handler)

**Finding:**  
Failed login attempts are not tracked. There is no exponential backoff, temporary
lockout, or CAPTCHA trigger. Combined with S-03, an attacker can run a credential
stuffing or dictionary attack undetected.

**Requirements:**

1. Track failed attempts in a new DB table or in-memory store (Redis or a simple
   Postgres table):
   ```sql
   CREATE TABLE login_attempts (
       ip        TEXT NOT NULL,
       email     TEXT NOT NULL,
       attempted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
   );
   CREATE INDEX ON login_attempts (email, attempted_at);
   ```

2. In the login handler, before verifying the password, count recent failures:
   ```rust
   let recent_failures: (i64,) = sqlx::query_as(
       "SELECT COUNT(*) FROM login_attempts
        WHERE email = $1 AND attempted_at > NOW() - INTERVAL '15 minutes'"
   )
   .bind(&body.email)
   .fetch_one(state.db()).await?;
   
   if recent_failures.0 >= 10 {
       return Err(AppError::BadRequest(
           "too many failed attempts — try again in 15 minutes".into()
       ));
   }
   ```

3. Insert a failure row when the password check fails; delete or ignore them on
   success (or let them age out naturally).

---

### S-05 — Missing HTTP Security Headers

**Location:** `src/main.rs:106–111` (no security headers layer)

**Finding:**  
No security headers are set on any response. This exposes users to:
- Clickjacking (`X-Frame-Options` / `frame-ancestors`)
- MIME-type confusion attacks (`X-Content-Type-Options`)
- Protocol downgrade after first visit (no `Strict-Transport-Security`)
- Overly permissive resource loading (no `Content-Security-Policy`)

**Requirements:**

1. Add `tower-http` `SetResponseHeadersLayer` (already a transitive dependency):
   ```rust
   use axum::http::{header, HeaderValue};
   use tower_http::set_header::SetResponseHeaderLayer;
   
   let security_headers = tower::ServiceBuilder::new()
       .layer(SetResponseHeaderLayer::if_not_present(
           header::HeaderName::from_static("x-content-type-options"),
           HeaderValue::from_static("nosniff"),
       ))
       .layer(SetResponseHeaderLayer::if_not_present(
           header::HeaderName::from_static("x-frame-options"),
           HeaderValue::from_static("DENY"),
       ))
       .layer(SetResponseHeaderLayer::if_not_present(
           header::HeaderName::from_static("referrer-policy"),
           HeaderValue::from_static("strict-origin-when-cross-origin"),
       ))
       .layer(SetResponseHeaderLayer::if_not_present(
           header::HeaderName::from_static("permissions-policy"),
           HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
       ));
   
   let app = Router::new()
       ...
       .layer(security_headers);
   ```

2. Add `Strict-Transport-Security` **only after TLS is confirmed working** (S-01).
   Premature HSTS on HTTP breaks the site:
   ```rust
   // Add only after Caddy/nginx TLS is verified:
   .layer(SetResponseHeaderLayer::if_not_present(
       header::STRICT_TRANSPORT_SECURITY,
       HeaderValue::from_static("max-age=63072000; includeSubDomains"),
   ))
   ```

3. Configure a `Content-Security-Policy` suited to the app's asset sources:
   ```
   Content-Security-Policy:
     default-src 'self';
     script-src 'self' https://cdn.jsdelivr.net;
     style-src 'self' https://fonts.googleapis.com;
     font-src 'self' https://fonts.gstatic.com;
     img-src 'self' data: https:;
     frame-ancestors 'none'
   ```
   Serve this header from the reverse proxy or from the Axum layer above.

---

### S-06 — No CSRF Protection on State-Changing Form POSTs

**Location:** `templates/layout_app.html:72` (logout form), all HTML form POSTs

**Finding:**  
The session cookie uses `SameSite::Lax`. Under Lax policy, cookies are sent on
top-level cross-site navigations (e.g., a link from an external page leading to a
POST). The logout form and any other `<form method="post">` in the HTML layer have
no CSRF token. A malicious external page could force-logout (or worse) a logged-in
user by linking to the endpoint.

**Requirements:**

1. Upgrade to `SameSite::Strict` in `src/main.rs` **after** confirming OAuth redirects
   work correctly. Note: `Strict` dropped the cookie in Google's cross-site redirect;
   this was already fixed. Re-test OAuth flow after this change:
   ```rust
   .with_same_site(SameSite::Strict)
   ```

2. For any HTML form that performs a destructive or sensitive action, embed a CSRF
   token stored in the session:
   ```rust
   // Generate on page render, store in session, verify on submit
   let csrf_token = generate_token(); // reuse existing pwd::generate_token()
   session.insert("csrf", &csrf_token).await?;
   ```
   Pass `csrf_token` into the Askama template and add a hidden field:
   ```html
   <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
   ```
   Validate it in the form handler before processing.

3. The JSON API (`/api/*`) endpoints are protected by the `Content-Type: application/json`
   requirement — a cross-origin form cannot set this header. Document this as the
   CSRF mitigation for API routes. No additional token is required for those.

---

## Medium

---

### S-07 — PostgreSQL Port Exposed to Host Network

**Location:** `docker-compose.yml:26–27`

**Finding:**  
```yaml
db:
  ports:
    - "5432:5432"   # binds 0.0.0.0:5432
```
The database is reachable from any machine on the host's network, not just from the
app container. If the host is a cloud VM with a misconfigured firewall, the DB is
internet-accessible.

**Requirements:**

1. Bind to loopback only for development, or remove the port mapping entirely for
   production (containers on the same Compose network communicate without port
   exposure):
   ```yaml
   db:
     # ports:      ← remove entirely for production
     #   - "5432:5432"
     # OR for dev-only local access:
     ports:
       - "127.0.0.1:5432:5432"
   ```

2. Ensure the host firewall (ufw, iptables, cloud security group) blocks port 5432
   from external traffic as a defense-in-depth measure.

---

### S-08 — Session Lifetime 7 Days With No Absolute Expiry

**Location:** `src/main.rs:42`

**Finding:**  
```rust
.with_expiry(Expiry::OnInactivity(Duration::days(7)))
```
A stolen session token (e.g., from a shared device, browser history, or network
intercept before TLS is added) remains valid for up to 7 days of inactivity. There
is no maximum absolute session lifetime.

**Requirements:**

1. Reduce inactivity timeout to 8–24 hours depending on acceptable UX:
   ```rust
   .with_expiry(Expiry::OnInactivity(Duration::hours(8)))
   ```

2. Optionally add an absolute cap by storing `session_created_at` in the session
   payload and rejecting it in the auth extractor if it exceeds the max age:
   ```rust
   // In session::save_user_id:
   session.insert("created_at", Utc::now().timestamp()).await?;
   
   // In CurrentUser extractor:
   let created_at: Option<i64> = session.get("created_at").await?;
   if let Some(ts) = created_at {
       let age = Utc::now().timestamp() - ts;
       if age > 86400 * 3 {  // 3-day absolute max
           session.flush().await.ok();
           return Err(...);
       }
   }
   ```

---

### S-09 — WrongPhase Error Leaks Internal State Labels

**Location:** `src/error.rs:28`, `src/routes/*/` (all callers of AppError::WrongPhase)

**Finding:**  
```rust
AppError::WrongPhase { required: String, current: String }
// → "wrong phase: action requires schedule or active, but reunion is in draft"
```
The full internal phase machine label is returned to the client in error responses.
This reveals implementation details about the reunion state machine to any
authenticated user.

**Requirements:**

1. Return a generic message to the client; keep detail in the server log only:
   ```rust
   AppError::WrongPhase { required, current } => {
       tracing::debug!("wrong phase: required={required}, current={current}");
       (StatusCode::CONFLICT, "this action is not available at the current stage".into())
   }
   ```

---

### S-10 — No Input Length Limits on Free-Text Fields

**Location:** Routes for comments (`activities.rs`), announcements (`announcements.rs`),
activity titles/descriptions, location notes, survey answers.

**Finding:**  
No maximum length validation is applied to user-supplied text before it reaches the
database. A single request with a multi-megabyte string payload (bounded only by
`DefaultBodyLimit::max(max_upload_bytes)` — currently 10 GB) can bloat the database
and degrade performance for all users.

**Requirements:**

1. Add length checks at the route handler level before any DB write. Suggested limits:
   | Field | Max |
   |-------|-----|
   | Announcement title | 200 chars |
   | Announcement content | 10,000 chars |
   | Activity title | 200 chars |
   | Activity description | 5,000 chars |
   | Comment content | 2,000 chars |
   | Location note | 500 chars |
   | Survey answer | 2,000 chars |

   Example pattern:
   ```rust
   if body.content.len() > 2_000 {
       return Err(AppError::BadRequest("comment exceeds 2,000 character limit".into()));
   }
   ```

2. Mirror these limits as `VARCHAR(n)` constraints in a schema migration so the
   database enforces them independently of application code.

3. Lower `DefaultBodyLimit` to a value appropriate for the largest legitimate payload
   (the media upload). This prevents a single connection from consuming 10 GB of
   memory/disk for a text-only request:
   ```rust
   // main.rs: keep the high limit only on the media upload route
   let app = Router::new()
       .nest("/api", api)
       .merge(pages_router())
       .layer(DefaultBodyLimit::max(64 * 1024))   // 64 KB default
       .layer(session_layer);
   // Apply the large limit only to the upload route:
   // .route("/:id/media", post(media::upload_media)
   //     .layer(DefaultBodyLimit::max(config.max_upload_bytes as usize)))
   ```

---

### S-11 — Mailpit Dev Container in Compose File

**Location:** `docker-compose.yml:38–44`

**Finding:**  
```yaml
mailpit:
  image: axllent/mailpit:latest
  ports:
    - "8025:8025"   # web UI — all emails visible here
    - "1025:1025"   # SMTP
```
If this service is started on a production host, all outgoing emails (verification
links, password reset tokens, announcement emails) are intercepted and visible in
the Mailpit web UI on port 8025.

**Requirements:**

1. Split the compose file: keep Mailpit only in a `docker-compose.dev.yml` override
   file. The base `docker-compose.yml` should target production and use real SMTP:
   ```bash
   # Development:
   docker compose -f docker-compose.yml -f docker-compose.dev.yml up
   # Production:
   docker compose up
   ```

2. Ensure `SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, and `SMTP_PASSWORD` in the
   production `.env` point to a real mail provider (Resend, Postmark, SES, etc.).

---

### S-12 — User-Controlled avatar_url Returned in All API Responses

**Location:** `src/models/user.rs:37` — `avatar_url` is serialized without restriction

**Finding:**  
The `avatar_url` field is stored as a raw URL string provided by the user (or pulled
from Google's profile). It is returned in every API response containing a `User`
object. A malicious user could set their avatar to a URL on an attacker-controlled
server, causing any client that renders the image (including admin's browser) to
beacon the request — leaking IP addresses and browser fingerprints.

**Requirements:**

1. Validate `avatar_url` on write: only permit URLs from a known allowlist of trusted
   image hosts, or require it to be a relative path to an internally hosted asset:
   ```rust
   // In update_me handler (auth.rs) and Google profile ingestion:
   fn is_allowed_avatar_url(url: &str) -> bool {
       let allowed = ["https://lh3.googleusercontent.com/", "https://avatars.githubusercontent.com/"];
       allowed.iter().any(|prefix| url.starts_with(prefix))
   }
   
   if let Some(url) = &body.avatar_url {
       if !url.is_empty() && !is_allowed_avatar_url(url) {
           return Err(AppError::BadRequest("avatar URL not from an allowed host".into()));
       }
   }
   ```

2. Alternatively, proxy avatar images through your own `/api/avatar/:user_id`
   endpoint that fetches and caches the image server-side, preventing the client
   from ever contacting third-party hosts directly.

---

## Low

---

### S-13 — .env File Must Not Be Committed to Version Control

**Location:** Project root `.env`

**Finding:**  
The `.env` file contains the live database password, session secret, and Google OAuth
client secret. If accidentally committed to a git repository (public or private) these
secrets are permanently exposed in git history even after deletion.

**Requirements:**

1. Confirm `.env` is in `.gitignore`:
   ```
   .env
   .env.*
   !.env.example
   ```

2. Add a `.env.example` file with all required keys but no real values, and commit it:
   ```
   DATABASE_URL=postgres://user:CHANGEME@db:5432/clanplan
   SESSION_SECRET=<generate with: openssl rand -hex 64>
   GOOGLE_CLIENT_ID=
   GOOGLE_CLIENT_SECRET=
   ADMIN_EMAIL=
   ADMIN_PASSWORD=<generate with: openssl rand -base64 32>
   APP_ENV=production
   SMTP_HOST=
   SMTP_USERNAME=
   SMTP_PASSWORD=
   ```

3. Audit git history for any prior accidental commit:
   ```bash
   git log --all --full-history -- .env
   ```
   If found, rotate all secrets immediately and use `git filter-repo` to purge history.

---

### S-14 — Media File Path Not Canonicalized Before Read

**Location:** `src/routes/media.rs` — file read/write operations

**Finding:**  
Uploaded files are stored as `{reunion_id}/{uuid_v4}.{ext}`, and the stored path is
joined to the base storage directory before reading. The UUID-based naming prevents
direct path traversal. However, there is no `.canonicalize()` check to confirm the
resolved path remains within the storage root, leaving a defense gap if a symlink
attack or future code change introduced a controllable path component.

**Requirements:**

1. After constructing the absolute path, canonicalize and verify it stays within the
   storage root:
   ```rust
   let storage_root = PathBuf::from(&state.config().media_storage_path)
       .canonicalize()
       .map_err(|e| AppError::Internal(anyhow::anyhow!("storage root: {e}")))?;
   
   let abs_path = storage_root.join(&media.file_path);
   let canonical = abs_path.canonicalize()
       .map_err(|_| AppError::NotFound)?;
   
   if !canonical.starts_with(&storage_root) {
       return Err(AppError::Forbidden);
   }
   
   let bytes = fs::read(&canonical).await
       .map_err(|e| AppError::Internal(anyhow::anyhow!("read file: {e}")))?;
   ```

---

### S-15 — OAuth CSRF Token Uses Non-Constant-Time Comparison

**Location:** `src/routes/auth.rs:326–329`

**Finding:**  
```rust
if stored_csrf != params.state {
    return Err(AppError::BadRequest("OAuth state mismatch".into()));
}
```
String comparison with `!=` short-circuits on the first differing byte, creating a
measurable timing difference that could theoretically be used to infer the CSRF token
value one byte at a time.

**Requirements:**

1. Use a constant-time comparison. The `subtle` crate is already a transitive
   dependency of the crypto stack:
   ```rust
   use subtle::ConstantTimeEq;
   
   let a = stored_csrf.as_bytes();
   let b = params.state.as_bytes();
   if a.len() != b.len() || a.ct_eq(b).unwrap_u8() == 0 {
       return Err(AppError::BadRequest("OAuth state mismatch".into()));
   }
   ```

---

## Implementation Roadmap

Address findings in this order to reduce risk fastest:

### Phase 1 — Before Go-Live (Blockers)
- [ ] **S-01** Deploy Caddy reverse proxy with automatic HTTPS
- [ ] **S-02** Set `ADMIN_EMAIL`, `ADMIN_PASSWORD`, `APP_ENV=production` in `.env`; add startup guard
- [ ] **S-13** Confirm `.env` is gitignored; add `.env.example`
- [ ] **S-11** Remove Mailpit from base `docker-compose.yml`
- [ ] **S-07** Remove `5432` port mapping from production compose

### Phase 2 — First Sprint Post-Launch
- [ ] **S-05** Add security headers layer to Axum router
- [ ] **S-03** Add `tower_governor` rate limiting to auth endpoints
- [ ] **S-04** Implement failed login tracking and lockout
- [ ] **S-08** Reduce session inactivity timeout to 8 hours; add `Secure` cookie flag

### Phase 3 — Second Sprint
- [ ] **S-06** Upgrade to `SameSite::Strict`; add CSRF tokens to HTML form POSTs
- [ ] **S-10** Add input length validation to all free-text handlers
- [ ] **S-09** Strip internal phase labels from WrongPhase error responses
- [ ] **S-12** Validate or proxy avatar URLs

### Phase 4 — Hardening
- [ ] **S-14** Add path canonicalization to media file operations
- [ ] **S-15** Replace OAuth CSRF comparison with constant-time equality

---

*This document should be reviewed and updated after each significant feature addition
or dependency upgrade.*
