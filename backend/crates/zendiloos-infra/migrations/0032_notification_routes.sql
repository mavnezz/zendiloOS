-- Notifications go through one mail server and are routed by kind
-- (docs/architecture.md, 4.3): an email channel names only its
-- recipients, and each kind of notification is kept in zendiloOS or not
-- and sent on the channels ticked for it.
--
-- Migrations are immutable once written; change the schema with a new file.

-- The mail server every email channel sends through; one row at most. The
-- password is readable by zendiloosd only, as the database is.
CREATE TABLE notification_mail_server (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    server     TEXT    NOT NULL,
    port       INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    security   TEXT    NOT NULL CHECK (security IN ('tls', 'starttls', 'none')),
    username   TEXT,
    password   TEXT,
    sender     TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- The server of the first email channel becomes the mail server.
INSERT INTO notification_mail_server (id, server, port, security, username, password, sender, updated_at)
SELECT 1,
       json_extract(settings, '$.server'),
       json_extract(settings, '$.port'),
       json_extract(settings, '$.security'),
       json_extract(settings, '$.username'),
       json_extract(settings, '$.password'),
       json_extract(settings, '$.from'),
       updated_at
FROM notification_channels
WHERE kind = 'email'
ORDER BY id
LIMIT 1;

-- Email channels keep their recipients only: addresses (`to`) and
-- zendiloOS accounts (`accounts`, none yet).
UPDATE notification_channels
SET settings = json_object('kind', 'email', 'to', json(json_extract(settings, '$.to')), 'accounts', json('[]'))
WHERE kind = 'email';

-- What was chosen per kind: kept in zendiloOS (no channel) or sent on a
-- channel. A kind without a row follows the defaults: kept, and sent on a
-- channel whose min_level carries the kind's usual level; so channels as
-- they were go on carrying what they carried.
CREATE TABLE notification_routes (
    kind       TEXT    NOT NULL,
    channel_id INTEGER REFERENCES notification_channels (id) ON DELETE CASCADE,
    enabled    INTEGER NOT NULL CHECK (enabled IN (0, 1))
) STRICT;

CREATE UNIQUE INDEX notification_routes_target ON notification_routes (kind, ifnull(channel_id, 0));
