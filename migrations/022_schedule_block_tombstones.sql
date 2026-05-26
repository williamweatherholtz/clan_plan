-- Tombstones for deleted schedule blocks, so the .ics export can emit
-- STATUS:CANCELLED VEVENTs that prompt calendar clients (Google, Apple,
-- Outlook, Thunderbird, Fantastical) to remove the previously-imported
-- event on re-fetch.
--
-- Why a separate table instead of a soft-delete column on schedule_blocks:
--   - Live queries stay simple — no WHERE deleted_at IS NULL boilerplate
--     in the hot paths (list_for_reunion, list_for_date, the today SSE
--     snapshot, the schedule page handler, the .ics active-event loop).
--   - The tombstone snapshot deliberately captures the LAST-KNOWN
--     date/time/title — DTSTART is required by RFC 5545 §3.6.1 for any
--     VEVENT, including cancellations, and many clients won't honor
--     STATUS:CANCELLED without it.
--   - Foreign-key cascades on reunion deletion still clean up tombstones.
--
-- Retention: tombstones persist indefinitely for now. A future migration
-- can prune rows whose reunion_date.end_date is more than (say) 90 days
-- past — by then every subscriber has either re-fetched or stopped caring.

CREATE TABLE cancelled_schedule_blocks (
    id           UUID         PRIMARY KEY,
    reunion_id   UUID         NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    block_date   DATE         NOT NULL,
    start_time   TIME         NOT NULL,
    end_time     TIME         NOT NULL,
    title        TEXT         NOT NULL,
    cancelled_at TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_cancelled_blocks_reunion
    ON cancelled_schedule_blocks (reunion_id);
