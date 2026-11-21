-- One account per email address; devices sign in with an emailed code.
CREATE TABLE accounts (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  created INTEGER NOT NULL
);

-- A device's session. Only the SHA-256 of its token is kept.
CREATE TABLE devices (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  created INTEGER NOT NULL,
  last_seen INTEGER NOT NULL
);
CREATE INDEX devices_account ON devices(account_id);

-- Sign-in codes: one live code per email, 10 minutes, 5 tries.
CREATE TABLE codes (
  email TEXT PRIMARY KEY,
  code_hash TEXT NOT NULL,
  expires INTEGER NOT NULL,
  tries INTEGER NOT NULL DEFAULT 0,
  sent INTEGER NOT NULL
);

-- Sends per email, for rate limiting (pruned as it is read).
CREATE TABLE code_sends (
  email TEXT NOT NULL,
  at INTEGER NOT NULL
);
CREATE INDEX code_sends_email ON code_sends(email, at);

-- Settings, pins and the like shared by an account's devices. Each key keeps
-- the time it was last changed (ms); the newest change wins.
CREATE TABLE settings (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value TEXT,
  updated INTEGER NOT NULL,
  device_id TEXT,
  PRIMARY KEY (account_id, key)
);
