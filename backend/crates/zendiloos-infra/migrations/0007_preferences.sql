-- Bounded context Identity: how each account wants the interface, so that
-- every browser it signs in with looks the same (docs/architecture.md, 4.2).
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE account_preferences (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    -- NULL: not chosen, each browser keeps what it shows.
    -- A language tag such as 'en'; zendiloos-identity knows which exist.
    language   TEXT CHECK (language GLOB '[a-z][a-z]'),
    theme      TEXT CHECK (theme IN ('light', 'dark', 'system'))
) STRICT;
