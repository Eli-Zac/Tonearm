-- Lookups and joins of Party codes that don't exist, for the guessing limit.
-- Parties themselves live in their PartyRoom Durable Object, not here.
CREATE TABLE party_fails (at INTEGER NOT NULL);
CREATE INDEX party_fails_at ON party_fails (at);
