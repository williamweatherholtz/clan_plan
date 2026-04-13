-- Tracks when a user has explicitly declared their expense entries complete
-- for a given reunion. Separate from having expenses logged — someone might
-- have paid nothing but still needs to confirm they're done.
CREATE TABLE expense_confirmations (
    reunion_id   UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    confirmed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (reunion_id, user_id)
);
