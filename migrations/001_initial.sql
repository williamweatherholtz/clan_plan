-- ── Enums ─────────────────────────────────────────────────────────────────────

CREATE TYPE reunion_phase AS ENUM (
    'draft',
    'availability',
    'date_selected',
    'locations',
    'location_selected',
    'schedule',
    'active',
    'post_reunion',
    'archived'
);

CREATE TYPE user_role AS ENUM (
    'sysadmin',
    'member'
);

CREATE TYPE block_type AS ENUM (
    'group',
    'optional',
    'meal',
    'travel'
);

CREATE TYPE activity_status AS ENUM (
    'proposed',
    'pinned',
    'scheduled',
    'cancelled'
);

-- ── Family Units ───────────────────────────────────────────────────────────────
-- A "family unit" is a household / branch of the family tree.
-- Members are associated with a unit; units are the granularity used for
-- the host-rotation tracker.

CREATE TABLE family_units (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Users ──────────────────────────────────────────────────────────────────────

CREATE TABLE users (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email               TEXT NOT NULL UNIQUE,
    display_name        TEXT NOT NULL,
    -- NULL when the account was created via Google OAuth only
    password_hash       TEXT,
    -- NULL for email/password accounts
    google_id           TEXT UNIQUE,
    family_unit_id      UUID REFERENCES family_units(id) ON DELETE SET NULL,
    role                user_role NOT NULL DEFAULT 'member',
    avatar_url          TEXT,
    email_verified_at   TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deactivated_at      TIMESTAMPTZ,
    -- Must have at least one authentication method
    CONSTRAINT at_least_one_auth CHECK (
        password_hash IS NOT NULL OR google_id IS NOT NULL
    )
);

CREATE TABLE email_verifications (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token       TEXT NOT NULL UNIQUE,
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE password_resets (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token       TEXT NOT NULL UNIQUE,
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Reunions ───────────────────────────────────────────────────────────────────

CREATE TABLE reunions (
    id                          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title                       TEXT NOT NULL,
    description                 TEXT,
    phase                       reunion_phase NOT NULL DEFAULT 'draft',
    responsible_admin_id        UUID REFERENCES users(id) ON DELETE SET NULL,
    -- Set after location vote is revealed and RA picks winner
    selected_location_id        UUID, -- FK added below after location_candidates is created
    -- Whether the RA has revealed location votes to all members
    location_votes_revealed     BOOLEAN NOT NULL DEFAULT FALSE,
    created_by                  UUID NOT NULL REFERENCES users(id),
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Date range is set by the RA when advancing from availability → date_selected
CREATE TABLE reunion_dates (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id  UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    start_date  DATE NOT NULL,
    end_date    DATE NOT NULL,
    set_by      UUID NOT NULL REFERENCES users(id),
    set_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT valid_date_range CHECK (end_date >= start_date)
);

-- ── Host Rotation ──────────────────────────────────────────────────────────────

CREATE TABLE host_rotation (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    family_unit_id  UUID NOT NULL REFERENCES family_units(id) ON DELETE CASCADE,
    -- NULL = designated next host but no reunion created yet
    reunion_id      UUID REFERENCES reunions(id) ON DELETE SET NULL,
    is_next         BOOLEAN NOT NULL DEFAULT FALSE,
    notes           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Only one family unit can be "next" at a time
CREATE UNIQUE INDEX idx_host_rotation_single_next
    ON host_rotation(is_next)
    WHERE is_next = TRUE;

-- ── Availability ───────────────────────────────────────────────────────────────
-- One row per (reunion, user, date) — present means "I'm available that day".
-- Members can add/remove rows until the RA closes the availability phase.

CREATE TABLE availability (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id      UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    available_date  DATE NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(reunion_id, user_id, available_date)
);

-- ── Location Candidates & Voting ───────────────────────────────────────────────

CREATE TABLE location_candidates (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id              UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    added_by                UUID NOT NULL REFERENCES users(id),
    title                   TEXT NOT NULL,
    description             TEXT,
    external_url            TEXT,           -- Airbnb, VRBO, etc.
    capacity                INTEGER,        -- max number of guests
    estimated_cost_cents    INTEGER,        -- in cents, NULL = unknown
    image_path              TEXT,           -- relative path under MEDIA_STORAGE_PATH
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Back-fill the FK now that the table exists
ALTER TABLE reunions
    ADD CONSTRAINT fk_reunions_selected_location
    FOREIGN KEY (selected_location_id)
    REFERENCES location_candidates(id)
    ON DELETE SET NULL;

-- Scale voting: 1 (not interested) – 5 (love it)
CREATE TABLE location_votes (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    location_candidate_id   UUID NOT NULL REFERENCES location_candidates(id) ON DELETE CASCADE,
    user_id                 UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    score                   SMALLINT NOT NULL CHECK (score BETWEEN 1 AND 5),
    comment                 TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(location_candidate_id, user_id)
);

-- ── Schedule ───────────────────────────────────────────────────────────────────
-- All time not covered by a block is implicitly free time.

CREATE TABLE schedule_blocks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id      UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    block_date      DATE NOT NULL,
    start_time      TIME NOT NULL,
    end_time        TIME NOT NULL,
    title           TEXT NOT NULL,
    description     TEXT,
    block_type      block_type NOT NULL DEFAULT 'optional',
    location_note   TEXT,
    created_by      UUID NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT valid_block_times CHECK (end_time > start_time)
);

-- Signup slots hang off schedule blocks (e.g. "Cook dinner — need 3 people")
CREATE TABLE signup_slots (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    schedule_block_id   UUID NOT NULL REFERENCES schedule_blocks(id) ON DELETE CASCADE,
    role_name           TEXT NOT NULL,
    description         TEXT,
    min_count           INTEGER NOT NULL DEFAULT 1,
    max_count           INTEGER,           -- NULL = unlimited
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT valid_counts CHECK (
        min_count >= 0
        AND (max_count IS NULL OR max_count >= min_count)
    )
);

CREATE TABLE signups (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    signup_slot_id  UUID NOT NULL REFERENCES signup_slots(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(signup_slot_id, user_id)
);

-- ── Activity Ideas ─────────────────────────────────────────────────────────────
-- Freeform suggestions open in any phase. Not phase-gated.
-- An idea can be promoted by the RA to a schedule_block.

CREATE TABLE activity_ideas (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id              UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    proposed_by             UUID NOT NULL REFERENCES users(id),
    title                   TEXT NOT NULL,
    description             TEXT,
    -- TRUE = idea needs a calendared time slot
    needs_time_slot         BOOLEAN NOT NULL DEFAULT FALSE,
    -- Loose suggestion before a block is assigned, e.g. "Saturday evening"
    suggested_time          TEXT,
    status                  activity_status NOT NULL DEFAULT 'proposed',
    -- Set by RA when promoting this idea to the schedule
    promoted_to_block_id    UUID REFERENCES schedule_blocks(id) ON DELETE SET NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Interest votes: 1 (meh) – 5 (absolutely)
CREATE TABLE activity_votes (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    activity_idea_id    UUID NOT NULL REFERENCES activity_ideas(id) ON DELETE CASCADE,
    user_id             UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    interest_score      SMALLINT NOT NULL CHECK (interest_score BETWEEN 1 AND 5),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(activity_idea_id, user_id)
);

CREATE TABLE activity_comments (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    activity_idea_id    UUID NOT NULL REFERENCES activity_ideas(id) ON DELETE CASCADE,
    user_id             UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content             TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Media ──────────────────────────────────────────────────────────────────────

CREATE TABLE media (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id          UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    uploaded_by         UUID NOT NULL REFERENCES users(id),
    -- UUID-based name used on disk (prevents path traversal / collisions)
    stored_filename     TEXT NOT NULL UNIQUE,
    original_filename   TEXT NOT NULL,
    mime_type           TEXT NOT NULL,
    file_size_bytes     BIGINT NOT NULL,
    -- Relative path under MEDIA_STORAGE_PATH
    file_path           TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Shared Expenses ────────────────────────────────────────────────────────────

CREATE TABLE expenses (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id      UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    logged_by       UUID NOT NULL REFERENCES users(id),
    paid_by_user_id UUID NOT NULL REFERENCES users(id),
    description     TEXT NOT NULL,
    -- Store cents to avoid floating-point issues
    amount_cents    INTEGER NOT NULL CHECK (amount_cents > 0),
    expense_date    DATE NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Each row = one person's share of one expense
CREATE TABLE expense_splits (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    expense_id      UUID NOT NULL REFERENCES expenses(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    amount_cents    INTEGER NOT NULL CHECK (amount_cents >= 0),
    UNIQUE(expense_id, user_id)
);

-- ── Announcements ──────────────────────────────────────────────────────────────

CREATE TABLE announcements (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id  UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    posted_by   UUID NOT NULL REFERENCES users(id),
    title       TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Feedback & Post-Reunion Survey ─────────────────────────────────────────────

-- Live freeform feedback (available during Active phase onward)
CREATE TABLE feedback (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id  UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content     TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- RA-defined questions for the post-reunion survey
CREATE TABLE survey_questions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id      UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    question_text   TEXT NOT NULL,
    order_index     INTEGER NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE survey_responses (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    survey_question_id  UUID NOT NULL REFERENCES survey_questions(id) ON DELETE CASCADE,
    user_id             UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    response_text       TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(survey_question_id, user_id)
);

-- ── Notifications ──────────────────────────────────────────────────────────────

CREATE TABLE notifications (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content     TEXT NOT NULL,
    link        TEXT,
    read_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Indexes ────────────────────────────────────────────────────────────────────

CREATE INDEX idx_users_email             ON users(email);
CREATE INDEX idx_users_google_id         ON users(google_id) WHERE google_id IS NOT NULL;
CREATE INDEX idx_availability_reunion    ON availability(reunion_id);
CREATE INDEX idx_availability_user       ON availability(user_id);
CREATE INDEX idx_location_votes_cand     ON location_votes(location_candidate_id);
CREATE INDEX idx_schedule_blocks_date    ON schedule_blocks(reunion_id, block_date);
CREATE INDEX idx_activity_ideas_reunion  ON activity_ideas(reunion_id);
CREATE INDEX idx_activity_votes          ON activity_votes(activity_idea_id);
CREATE INDEX idx_activity_comments       ON activity_comments(activity_idea_id);
CREATE INDEX idx_signups_slot            ON signups(signup_slot_id);
CREATE INDEX idx_media_reunion           ON media(reunion_id);
CREATE INDEX idx_expenses_reunion        ON expenses(reunion_id);
CREATE INDEX idx_notifications_user      ON notifications(user_id, read_at);
