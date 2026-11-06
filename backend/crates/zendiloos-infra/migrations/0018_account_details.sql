-- More about each account: a description and an e-mail address, whether
-- only administrators set its password, and when it was last set; and the
-- password policy (docs/architecture.md, 4.2). One policy row at most;
-- without it, the defaults: at least 10 characters, nothing else.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE accounts ADD COLUMN description TEXT;
ALTER TABLE accounts ADD COLUMN email TEXT;
ALTER TABLE accounts ADD COLUMN password_locked INTEGER NOT NULL DEFAULT 0 CHECK (password_locked IN (0, 1));
ALTER TABLE accounts ADD COLUMN password_changed_at INTEGER NOT NULL DEFAULT 0;
UPDATE accounts SET password_changed_at = updated_at;

CREATE TABLE password_policy (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    min_length   INTEGER NOT NULL CHECK (min_length BETWEEN 10 AND 64),
    mixed_case   INTEGER NOT NULL CHECK (mixed_case IN (0, 1)),
    digit        INTEGER NOT NULL CHECK (digit IN (0, 1)),
    symbol       INTEGER NOT NULL CHECK (symbol IN (0, 1)),
    max_age_days INTEGER CHECK (max_age_days IS NULL OR max_age_days BETWEEN 30 AND 730)
) STRICT;
