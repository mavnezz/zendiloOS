-- How far each account has read and cleared the notifications, so that
-- every browser of the account shows the same (docs/architecture.md, 3.1).
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE notification_marks (
    account_id    INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    -- Notification IDs; everything up to them is read, or no longer listed.
    read_up_to    INTEGER NOT NULL DEFAULT 0 CHECK (read_up_to >= 0),
    cleared_up_to INTEGER NOT NULL DEFAULT 0 CHECK (cleared_up_to >= 0)
) STRICT;
