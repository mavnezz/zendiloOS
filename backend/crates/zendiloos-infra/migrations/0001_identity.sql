-- Bounded context Identity: accounts, login sessions and the audit log.
--
-- Migrations are immutable once written; change the schema with a new file.
-- Timestamps are Unix seconds (UTC). Domain rules are enforced in
-- zendiloos-identity; the checks here are a last line of defence.

-- One account per person, valid for the web interface, SMB and the file
-- manager (docs/architecture.md, 4.2).
CREATE TABLE accounts (
    -- AUTOINCREMENT: an ID is never reused, so the audit log keeps pointing
    -- at the account that acted, even after it was deleted.
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Login name and Linux account name.
    name                     TEXT    NOT NULL UNIQUE CHECK (name <> '' AND name = lower(name)),
    full_name                TEXT,
    role                     TEXT    NOT NULL CHECK (role IN ('admin', 'user', 'readonly')),
    -- Argon2id in PHC string format.
    password_hash            TEXT    NOT NULL,
    password_change_required INTEGER NOT NULL DEFAULT 0 CHECK (password_change_required IN (0, 1)),
    disabled                 INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    -- Set once the agent has created the Linux account.
    linux_uid                INTEGER UNIQUE,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL
) STRICT;

-- Sessions are found by a SHA-256 digest of the cookie token; the token
-- itself is never stored.
CREATE TABLE sessions (
    id           INTEGER PRIMARY KEY,
    token_hash   BLOB    NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    account_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    csrf_token   TEXT    NOT NULL,
    client_ip    TEXT,
    user_agent   TEXT,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    -- End of the absolute lifetime; the idle timeout uses last_seen_at.
    expires_at   INTEGER NOT NULL
) STRICT;

CREATE INDEX sessions_account_id ON sessions (account_id);

-- Who did what, when. Rows are history: they are never changed, and
-- retention may delete old ones. Hence no foreign key on account_id, which
-- would have to rewrite rows when an account is deleted.
CREATE TABLE audit_log (
    id         INTEGER PRIMARY KEY,
    at         INTEGER NOT NULL,
    account_id INTEGER,
    -- Account name at the time; for failed logins the name that was tried.
    actor      TEXT,
    client_ip  TEXT,
    -- Dotted name, e.g. 'identity.login'.
    action     TEXT    NOT NULL,
    target     TEXT,
    outcome    TEXT    NOT NULL CHECK (outcome IN ('success', 'failure', 'denied')),
    request_id TEXT,
    -- JSON object with further context. Never secrets.
    details    TEXT    CHECK (details IS NULL OR json_valid(details))
) STRICT;

CREATE INDEX audit_log_at ON audit_log (at);

CREATE TRIGGER audit_log_append_only
BEFORE UPDATE ON audit_log
BEGIN
    SELECT RAISE(ABORT, 'audit_log is append-only');
END;
