-- Extend activity_rsvps with a role so meal activities can have separate
-- "I'll make" and "I'll cleanup" sign-ups. Non-meal activities continue to
-- use the default 'in' role (the original "I'm in" commitment).

ALTER TABLE activity_rsvps
    ADD COLUMN role TEXT NOT NULL DEFAULT 'in'
    CONSTRAINT activity_rsvps_role_valid
        CHECK (role IN ('in', 'make', 'cleanup'));

-- One person can sign up for multiple roles on the same meal idea
-- (e.g. both 'make' and 'cleanup'), so the PK now includes role.
ALTER TABLE activity_rsvps DROP CONSTRAINT activity_rsvps_pkey;
ALTER TABLE activity_rsvps ADD PRIMARY KEY (activity_idea_id, user_id, role);
