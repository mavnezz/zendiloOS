-- The network settings a restored configuration backup brought
-- (zendiloos-network, docs/architecture.md, 4.15): the systemd-networkd
-- files the backed-up machine's settings kept, set aside at the first start
-- after the restore, so they are not taken for this machine's files changed
-- outside zendiloOS. The Network page offers them interface by interface,
-- each applied as an ordinary change that goes back unless confirmed; never
-- by themselves. Forgotten when the administrator says so.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE network_restored (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    -- When they were set aside.
    restored_at INTEGER NOT NULL
) STRICT;

-- `05-zendiloos-<name>.network` or `.netdev`, with its contents.
CREATE TABLE network_restored_files (
    name TEXT PRIMARY KEY CHECK (name GLOB '05-zendiloos-*' AND length(name) <= 64),
    text TEXT NOT NULL CHECK (length(text) <= 65536)
) STRICT;
