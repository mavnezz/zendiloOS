-- The desktop, as each account arranged it: its background, the widgets
-- beside it and the order of the app icons (docs/architecture.md, 4.2).
-- NULL: not chosen. Layouts are ids joined by commas.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE account_preferences ADD COLUMN wallpaper TEXT CHECK (wallpaper GLOB '[a-z]*');
ALTER TABLE account_preferences ADD COLUMN widgets TEXT;
ALTER TABLE account_preferences ADD COLUMN desktop_icons TEXT;
