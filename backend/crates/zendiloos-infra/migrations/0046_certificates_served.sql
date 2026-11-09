-- Whether nginx took the pair last issued of a certificate the web
-- interface shows, or a change waiting would (zendiloos-certificates,
-- docs/architecture.md, 4.11). 0: its reload failed, so nginx still serves
-- the one before; the daily look has nginx take it, rather than the
-- renewal counting as done.
ALTER TABLE certificates ADD COLUMN served INTEGER NOT NULL DEFAULT 1 CHECK (served IN (0, 1));
