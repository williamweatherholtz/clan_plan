-- Explicit "I'm in!" commitment on an activity idea, separate from the 1-5 interest vote.
CREATE TABLE activity_rsvps (
    activity_idea_id UUID NOT NULL REFERENCES activity_ideas(id) ON DELETE CASCADE,
    user_id          UUID NOT NULL REFERENCES users(id)          ON DELETE CASCADE,
    rsvp_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (activity_idea_id, user_id)
);
