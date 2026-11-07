-- Bounded context Sharing: a share being deleted. It is offered no more
-- and left out everywhere, but stays recorded until its dataset is gone,
-- so a deletion that fails halfway is tried again rather than leaving a
-- dataset no share knows (docs/architecture.md, 4.4).
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE shares ADD COLUMN deleting INTEGER NOT NULL DEFAULT 0 CHECK (deleting IN (0, 1));
