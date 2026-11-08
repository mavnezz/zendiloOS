-- Bounded context Identity: the second factor of each account's sign-in to
-- the web interface (docs/architecture.md, 4.2). The authenticator app's
-- secret, the last time step a code was taken for (none is taken twice),
-- and the recovery codes, as digests only. SMB and NFS do not use it.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE account_two_factor (
    account_id     INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    -- Base32 (RFC 4648), 160 bits. NULL: not on.
    secret         TEXT CHECK (length(secret) = 32),
    -- A secret being set up, waiting for its first code.
    pending_secret TEXT CHECK (length(pending_secret) = 32),
    -- The time step (unix seconds / 30) the last code taken belongs to.
    last_step      INTEGER CHECK (last_step >= 0),
    -- SHA-256 digests (hex) of the recovery codes not used yet, one per line.
    recovery_codes TEXT    NOT NULL DEFAULT '',
    -- When it was turned on.
    enabled_at     INTEGER,
    CHECK ((secret IS NULL) = (enabled_at IS NULL))
) STRICT;
