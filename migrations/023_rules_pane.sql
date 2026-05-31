-- House Rules pane: a per-reunion long-form doc the RA can edit, with a
-- comment thread anyone can post to. The label is per-reunion so each
-- group can call it what fits ("House Rules", "Ground Rules", "Logistics",
-- "Charter", etc.) without a code change.
--
-- Why columns on `reunions` and not a separate table:
--   The body is 1:1 with reunion. A side table would force a LEFT JOIN
--   on every page load just to surface the tab label (which is needed
--   for the nav bar on every reunion subpage).
--
-- Why `rules_label NOT NULL DEFAULT`:
--   The nav-tab builder reads the label unconditionally; a NULL would
--   require an Option<String> + an unwrap_or in Rust, which is just
--   moving the default to the wrong layer.

ALTER TABLE reunions
    ADD COLUMN rules_label TEXT NOT NULL DEFAULT 'House Rules',
    ADD COLUMN rules_body  TEXT;

-- One comment thread per reunion. Mirrors activity_comments shape so the
-- existing templates / patterns transfer cleanly. CASCADE on user delete
-- so account removals don't leave orphan rows.
CREATE TABLE rules_comments (
    id          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id  UUID         NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    user_id     UUID         NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    content     TEXT         NOT NULL,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_rules_comments_reunion ON rules_comments(reunion_id, created_at);
