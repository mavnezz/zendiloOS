-- Whether the UPS's call for a new battery was told (zendiloos-ups,
-- docs/architecture.md, 4.16): kept, so that it is told once and again
-- only after it was gone, not at every start of zendiloosd.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE ups_settings ADD COLUMN replace_battery_told INTEGER NOT NULL DEFAULT 0;
