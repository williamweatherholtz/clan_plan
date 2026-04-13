-- App-wide settings: single row enforced by the CHECK constraint.
CREATE TABLE app_settings (
    id                   INTEGER PRIMARY KEY DEFAULT 1,
    registration_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT singleton CHECK (id = 1)
);

INSERT INTO app_settings (registration_enabled) VALUES (FALSE);
