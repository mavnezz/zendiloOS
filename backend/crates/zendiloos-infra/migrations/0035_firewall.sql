-- The firewall (zendiloos-firewall): its settings, the rules in their
-- order, and the services a notification said the rules block.

-- One row. No row: off, and everything allowed once turned on.
CREATE TABLE firewall_settings (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    enabled  INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    -- What happens to a connection no rule matches.
    fallback TEXT    NOT NULL CHECK (fallback IN ('allow', 'deny'))
) STRICT;

CREATE TABLE firewall_rules (
    -- From 0: the first rule that matches decides.
    position  INTEGER PRIMARY KEY CHECK (position >= 0),
    enabled   INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    -- NULL: every interface.
    interface TEXT,
    -- 'all', one of the NAS's services ('web_interface', 'smb', 'nfs',
    -- 'ssh', 'discovery', 'ping'), 'project' or 'ports'.
    service   TEXT    NOT NULL,
    -- The Docker project's name, for 'project'.
    project   TEXT,
    -- For 'ports': which protocol, and the ports as written ('80, 8000-8100').
    protocol  TEXT CHECK (protocol IN ('tcp', 'udp', 'both')),
    ports     TEXT,
    -- 'any', an address, a network ('192.168.1.0/24') or a range ('a-b').
    source    TEXT    NOT NULL,
    action    TEXT    NOT NULL CHECK (action IN ('allow', 'deny')),
    CHECK ((service = 'project') = (project IS NOT NULL)),
    CHECK ((service = 'ports') = (protocol IS NOT NULL AND ports IS NOT NULL))
) STRICT;

-- Services a notification said nobody reaches, by key ('smb',
-- 'project:<name>'): told once until they are reachable again.
CREATE TABLE firewall_blocked (
    service TEXT PRIMARY KEY
) STRICT;
