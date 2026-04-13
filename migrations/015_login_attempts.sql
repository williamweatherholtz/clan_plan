-- Tracks failed login attempts for rate limiting and lockout (S-03, S-04).
CREATE TABLE login_attempts (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    ip           TEXT NOT NULL,
    email        TEXT NOT NULL,
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX login_attempts_email_time ON login_attempts (email, attempted_at);
CREATE INDEX login_attempts_ip_time    ON login_attempts (ip, attempted_at);
