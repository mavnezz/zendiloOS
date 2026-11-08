-- Two more of each account's interface choices (docs/architecture.md, 4.2).
--
-- The reminder to star zendiloOS on GitHub, which administrators see at
-- most twice: how often it was shown, when last (Unix seconds; NULL: never)
-- and what they answered (NULL: not yet). Whether it is due is the
-- interface's business; nothing of it leaves the machine.
--
-- A desktop background of one colour: the wallpaper named 'color', in the
-- colour kept here as '#rrggbb', lowercase. Kept when another background
-- is chosen, so the colour is still there when going back to it.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE account_preferences ADD COLUMN star_prompt_shown INTEGER NOT NULL DEFAULT 0
    CHECK (star_prompt_shown BETWEEN 0 AND 2);
ALTER TABLE account_preferences ADD COLUMN star_prompt_shown_at INTEGER CHECK (star_prompt_shown_at >= 0);
ALTER TABLE account_preferences ADD COLUMN star_prompt_answer TEXT
    CHECK (star_prompt_answer IN ('later', 'starred', 'declined'));
ALTER TABLE account_preferences ADD COLUMN wallpaper_color TEXT
    CHECK (wallpaper_color GLOB '#[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]');
