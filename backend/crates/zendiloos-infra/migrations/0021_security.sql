-- The web interface's defences (docs/architecture.md, 4.2): when idle
-- sessions end, when accounts lock and addresses are blocked after failed
-- logins, the addresses never blocked, and the blocks. One settings row at
-- most; without it, the defaults.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE security_settings (
    id                   INTEGER PRIMARY KEY CHECK (id = 1),
    idle_minutes         INTEGER NOT NULL CHECK (idle_minutes BETWEEN 5 AND 1440),
    -- Both NULL: accounts do not lock.
    lock_failures        INTEGER CHECK (lock_failures BETWEEN 3 AND 100),
    lock_window_minutes  INTEGER CHECK (lock_window_minutes BETWEEN 1 AND 1440),
    -- Both NULL: no address is blocked automatically.
    block_failures       INTEGER CHECK (block_failures BETWEEN 3 AND 100),
    block_window_minutes INTEGER CHECK (block_window_minutes BETWEEN 1 AND 1440),
    -- NULL: blocks last until lifted by hand.
    block_release_days   INTEGER CHECK (block_release_days BETWEEN 1 AND 365),
    CHECK ((lock_failures IS NULL) = (lock_window_minutes IS NULL)),
    CHECK ((block_failures IS NULL) = (block_window_minutes IS NULL))
) STRICT;

-- Never blocked: an address or a network, as zendiloos-identity writes it
-- (`192.168.1.0/24`); in the order they were given.
CREATE TABLE allowed_addresses (
    address TEXT PRIMARY KEY
) STRICT;

CREATE TABLE blocked_addresses (
    address    TEXT    PRIMARY KEY,
    blocked_at INTEGER NOT NULL,
    -- NULL: until lifted by hand.
    until      INTEGER,
    -- 1: after failed logins; 0: by an administrator.
    automatic  INTEGER NOT NULL CHECK (automatic IN (0, 1))
) STRICT;
