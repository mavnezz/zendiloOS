-- The firewall's settings a restored configuration backup brought
-- (zendiloos-firewall, docs/architecture.md, 4.12 and 4.15), set aside at
-- the first start after the restore instead of being put in force without
-- confirmation: rules made for another network could shut the
-- administrator out. This machine starts with the firewall's defaults (off,
-- as on a fresh installation); the Firewall tab offers the backup's
-- settings, applied as an ordinary change that goes back unless confirmed.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE firewall_restored (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    -- When they were set aside.
    restored_at INTEGER NOT NULL,
    enabled     INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    fallback    TEXT    NOT NULL CHECK (fallback IN ('allow', 'deny'))
) STRICT;

-- As `firewall_rules`.
CREATE TABLE firewall_restored_rules (
    position  INTEGER PRIMARY KEY CHECK (position >= 0),
    enabled   INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    interface TEXT,
    service   TEXT    NOT NULL,
    project   TEXT,
    protocol  TEXT CHECK (protocol IN ('tcp', 'udp', 'both')),
    ports     TEXT,
    source    TEXT    NOT NULL,
    action    TEXT    NOT NULL CHECK (action IN ('allow', 'deny')),
    CHECK ((service = 'project') = (project IS NOT NULL)),
    CHECK ((service = 'ports') = (protocol IS NOT NULL AND ports IS NOT NULL))
) STRICT;
