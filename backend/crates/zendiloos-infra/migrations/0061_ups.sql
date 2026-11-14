-- The UPS's settings (zendiloos-ups, docs/architecture.md, 4.16): where the
-- UPS is, when the NAS shuts down on battery, whether the UPS cuts its
-- output afterwards, and the account other devices follow it with; one
-- row, the settings as a JSON document (the kinds of UPS have little in
-- common).
--
-- The passwords are kept as they are, as the mail server's is: another
-- device's NUT account and the account for the devices that follow (in
-- the document), and the one of the NAS's own account at NUT's server,
-- which is made with the row and never shown. The agent writes them into
-- NUT's files, which want them in plain text; the database is readable by
-- zendiloosd only.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE ups_settings (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    settings       TEXT    NOT NULL,
    local_password TEXT    NOT NULL,
    updated_at     INTEGER NOT NULL
) STRICT;
