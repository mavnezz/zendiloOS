-- When disks test themselves (docs/architecture.md, 5): one plan per kind
-- of test, the local hour it starts from, and the disks by serial number
-- (none: every disk that can).
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE self_test_plans (
    kind      TEXT    PRIMARY KEY CHECK (kind IN ('short', 'long')),
    frequency TEXT    NOT NULL CHECK (frequency IN ('daily', 'weekly', 'monthly')),
    hour      INTEGER NOT NULL CHECK (hour BETWEEN 0 AND 23),
    -- Unix seconds of the last run; NULL until the first.
    last_run  INTEGER
) STRICT;

CREATE TABLE self_test_plan_disks (
    kind   TEXT NOT NULL REFERENCES self_test_plans (kind) ON DELETE CASCADE,
    serial TEXT NOT NULL,
    PRIMARY KEY (kind, serial)
) STRICT;
