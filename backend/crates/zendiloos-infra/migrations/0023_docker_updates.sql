-- Bounded context Containers: when newer images are looked for and taken
-- (docs/architecture.md, 4.9). One row at most; without it, the defaults:
-- looked for, taken by hand.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE docker_update_settings (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    check_images INTEGER NOT NULL CHECK (check_images IN (0, 1)),
    -- Bit 0 is Monday; NULL with minute: no schedule.
    weekdays     INTEGER CHECK (weekdays BETWEEN 1 AND 127),
    -- Minutes after midnight in the machine's time zone.
    minute       INTEGER CHECK (minute BETWEEN 0 AND 1439),
    CHECK ((weekdays IS NULL) = (minute IS NULL))
) STRICT;
