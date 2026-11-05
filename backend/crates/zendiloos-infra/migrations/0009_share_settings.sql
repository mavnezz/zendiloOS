-- How each share's dataset stores data (docs/architecture.md, 5): a quota
-- on its own data (refquota), compression and record size. The database
-- is the source; the datasets get these at every change and at startup.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE shares ADD COLUMN quota_bytes INTEGER CHECK (quota_bytes >= 1073741824);
ALTER TABLE shares ADD COLUMN compression TEXT NOT NULL DEFAULT 'lz4' CHECK (compression IN ('off', 'lz4', 'zstd'));
ALTER TABLE shares ADD COLUMN record_size INTEGER NOT NULL DEFAULT 131072
    CHECK (record_size IN (16384, 131072, 1048576));
