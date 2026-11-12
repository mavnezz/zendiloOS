-- zendiloOS's network settings as last confirmed (zendiloos-network,
-- docs/architecture.md, section 6, Network): its systemd-networkd files as
-- they were once a change was confirmed, which zendiloosd compares with the
-- files at its start. Until the first start with this table the files are
-- the truth; that start keeps them as they are.
CREATE TABLE network_settings (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    -- When the settings were confirmed, or taken from the files.
    kept_at INTEGER NOT NULL,
    -- How the files differed when that was last told as a notification,
    -- so each difference is told once; NULL when nothing was told.
    said    TEXT
) STRICT;

-- `05-zendiloos-<name>.network` or `.netdev`, with its contents.
CREATE TABLE network_setting_files (
    name TEXT PRIMARY KEY CHECK (name GLOB '05-zendiloos-*' AND length(name) <= 64),
    text TEXT NOT NULL CHECK (length(text) <= 65536)
) STRICT;
