-- Pairing: a signed-in device shows a short code (or QR), a new device claims
-- it and is signed in to the same account. One use, five minutes.
CREATE TABLE pair_codes (
  code_hash TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  expires INTEGER NOT NULL
);
CREATE INDEX pair_codes_account ON pair_codes(account_id);

-- Wrong guesses, so the codes can't be brute forced (pruned as read).
CREATE TABLE pair_fails (
  at INTEGER NOT NULL
);
