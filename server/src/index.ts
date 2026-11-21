// Tonearm's account server: email-code sign-in, a device list, and settings
// shared by an account's devices. Plain JSON over HTTPS under /v1.
import { sendMail, type MailEnv } from "./mail";

interface Env extends MailEnv {
  DB: any; // D1Database
  BASE_HOST: string;
}

const CODE_TTL_MS = 10 * 60_000;
const CODE_TRIES = 5;
const CODE_GAP_MS = 30_000; // least time between codes to one address
const CODES_PER_HOUR = 5;
const SEEN_EVERY_MS = 5 * 60_000;
const MAX_BODY = 256 * 1024;
const MAX_KEY = 128;
const MAX_VALUE = 16 * 1024;
const MAX_CHANGES = 100;
const MAX_KEYS = 500;

class Fail extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });

async function sha256(text: string): Promise<string> {
  const bytes = new TextEncoder().encode(text);
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return [...hash].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function token(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

function sixDigits(): string {
  // Rejection sampling, so every code is equally likely.
  const limit = 4294967296 - (4294967296 % 1000000);
  for (;;) {
    const n = crypto.getRandomValues(new Uint32Array(1))[0];
    if (n < limit) return String(n % 1000000).padStart(6, "0");
  }
}

function sameText(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

async function body(req: Request): Promise<any> {
  const length = Number(req.headers.get("content-length") ?? 0);
  if (length > MAX_BODY) throw new Fail(413, "too large");
  try {
    const value = await req.json();
    if (value && typeof value === "object") return value;
  } catch {}
  throw new Fail(400, "send a JSON object");
}

function cleanEmail(value: unknown): string {
  const email = String(value ?? "").trim().toLowerCase();
  if (email.length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
    throw new Fail(400, "that doesn't look like an email address");
  }
  return email;
}

interface Session {
  account_id: string;
  device_id: string;
  email: string;
}

async function session(req: Request, env: Env): Promise<Session> {
  const header = req.headers.get("authorization") ?? "";
  const given = header.startsWith("Bearer ") ? header.slice(7).trim() : "";
  if (!given) throw new Fail(401, "sign in first");
  const row = await env.DB.prepare(
    `SELECT d.id AS device_id, d.account_id, d.last_seen, a.email
       FROM devices d JOIN accounts a ON a.id = d.account_id
      WHERE d.token_hash = ?`,
  )
    .bind(await sha256(given))
    .first();
  if (!row) throw new Fail(401, "this device is signed out");
  const now = Date.now();
  if (now - row.last_seen > SEEN_EVERY_MS) {
    await env.DB.prepare("UPDATE devices SET last_seen = ? WHERE id = ?")
      .bind(now, row.device_id)
      .run();
  }
  return { account_id: row.account_id, device_id: row.device_id, email: row.email };
}

// POST /v1/auth/code {email}
async function requestCode(req: Request, env: Env): Promise<Response> {
  const email = cleanEmail((await body(req)).email);
  const now = Date.now();

  await env.DB.prepare("DELETE FROM code_sends WHERE at < ?").bind(now - 3_600_000).run();
  const recent = await env.DB.prepare(
    "SELECT COUNT(*) AS n, MAX(at) AS last FROM code_sends WHERE email = ?",
  )
    .bind(email)
    .first();
  if (recent.n >= CODES_PER_HOUR) throw new Fail(429, "too many codes; try again later");
  if (recent.last && now - recent.last < CODE_GAP_MS) {
    throw new Fail(429, "a code was just sent; wait a moment");
  }

  const code = sixDigits();
  await env.DB.batch([
    env.DB.prepare(
      `INSERT INTO codes (email, code_hash, expires, tries, sent) VALUES (?, ?, ?, 0, ?)
       ON CONFLICT(email) DO UPDATE SET code_hash = excluded.code_hash,
         expires = excluded.expires, tries = 0, sent = excluded.sent`,
    ).bind(email, await sha256(`${code}:${email}`), now + CODE_TTL_MS, now),
    env.DB.prepare("INSERT INTO code_sends (email, at) VALUES (?, ?)").bind(email, now),
  ]);

  try {
    await sendMail(
      env,
      email,
      "Your Tonearm sign-in code",
      `Your Tonearm code is ${code}.\n\nIt works for 10 minutes. If you didn't ask for it, ignore this email.`,
    );
  } catch (e) {
    console.error("sending the sign-in code failed:", e);
    throw new Fail(502, "couldn't send the email; try again");
  }
  return json({ ok: true, expires_in: CODE_TTL_MS / 1000 });
}

// POST /v1/auth/verify {email, code, device}
async function verifyCode(req: Request, env: Env): Promise<Response> {
  const input = await body(req);
  const email = cleanEmail(input.email);
  const code = String(input.code ?? "").replace(/\s/g, "");
  const name = String(input.device ?? "").trim().slice(0, 60) || "Device";
  const now = Date.now();
  const wrong = new Fail(400, "that code isn't right or has expired");

  const row = await env.DB.prepare("SELECT * FROM codes WHERE email = ?").bind(email).first();
  if (!row || row.expires < now || row.tries >= CODE_TRIES) throw wrong;
  await env.DB.prepare("UPDATE codes SET tries = tries + 1 WHERE email = ?").bind(email).run();
  if (!sameText(row.code_hash, await sha256(`${code}:${email}`))) throw wrong;
  await env.DB.prepare("DELETE FROM codes WHERE email = ?").bind(email).run();

  let account = await env.DB.prepare("SELECT id FROM accounts WHERE email = ?").bind(email).first();
  if (!account) {
    const id = crypto.randomUUID();
    await env.DB.prepare("INSERT INTO accounts (id, email, created) VALUES (?, ?, ?)")
      .bind(id, email, now)
      .run();
    account = { id };
  }
  const deviceId = crypto.randomUUID();
  const secret = token();
  await env.DB.prepare(
    "INSERT INTO devices (id, account_id, name, token_hash, created, last_seen) VALUES (?, ?, ?, ?, ?, ?)",
  )
    .bind(deviceId, account.id, name, await sha256(secret), now, now)
    .run();
  return json({ token: secret, account_id: account.id, device_id: deviceId, email });
}

const PAIR_TTL_MS = 5 * 60_000;
const PAIR_LIVE_MAX = 5; // codes an account can have open at once
const PAIR_FAILS_PER_MINUTE = 30; // wrong guesses, across everyone
// 31 letters and digits with no look-alikes: 8 of them are about 40 bits.
const PAIR_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";

function pairCode(): string {
  const limit = 256 - (256 % PAIR_ALPHABET.length);
  let out = "";
  while (out.length < 8) {
    for (const b of crypto.getRandomValues(new Uint8Array(16))) {
      if (b < limit && out.length < 8) out += PAIR_ALPHABET[b % PAIR_ALPHABET.length];
    }
  }
  return out;
}

const pairKey = (code: string) => sha256(`pair:${code.toUpperCase().replace(/[^A-Z0-9]/g, "")}`);

// POST /v1/pair/start  (signed in)  ->  {code, expires_in}
async function startPairing(s: Session, env: Env): Promise<Response> {
  const now = Date.now();
  await env.DB.prepare("DELETE FROM pair_codes WHERE expires < ?").bind(now).run();
  const live = await env.DB.prepare("SELECT COUNT(*) AS n FROM pair_codes WHERE account_id = ?")
    .bind(s.account_id)
    .first();
  if (live.n >= PAIR_LIVE_MAX) throw new Fail(429, "too many open pairing codes; wait a few minutes");
  const code = pairCode();
  await env.DB.prepare("INSERT INTO pair_codes (code_hash, account_id, expires) VALUES (?, ?, ?)")
    .bind(await pairKey(code), s.account_id, now + PAIR_TTL_MS)
    .run();
  return json({ code: `${code.slice(0, 4)}-${code.slice(4)}`, expires_in: PAIR_TTL_MS / 1000 });
}

// POST /v1/pair/claim {code, device}  ->  the same answer as /v1/auth/verify
async function claimPairing(req: Request, env: Env): Promise<Response> {
  const input = await body(req);
  const name = String(input.device ?? "").trim().slice(0, 60) || "Device";
  const now = Date.now();
  const wrong = new Fail(400, "that pairing code isn't right or has expired");

  await env.DB.prepare("DELETE FROM pair_fails WHERE at < ?").bind(now - 60_000).run();
  const fails = await env.DB.prepare("SELECT COUNT(*) AS n FROM pair_fails").first();
  if (fails.n >= PAIR_FAILS_PER_MINUTE) throw new Fail(429, "too many wrong codes; wait a minute");

  const hash = await pairKey(String(input.code ?? ""));
  // Taking the code and deleting it is one statement, so two devices can't both use it.
  const taken = await env.DB.prepare(
    "DELETE FROM pair_codes WHERE code_hash = ? AND expires >= ? RETURNING account_id",
  )
    .bind(hash, now)
    .first();
  if (!taken) {
    await env.DB.prepare("INSERT INTO pair_fails (at) VALUES (?)").bind(now).run();
    throw wrong;
  }

  const account = await env.DB.prepare("SELECT id, email FROM accounts WHERE id = ?")
    .bind(taken.account_id)
    .first();
  if (!account) throw wrong;
  const deviceId = crypto.randomUUID();
  const secret = token();
  await env.DB.prepare(
    "INSERT INTO devices (id, account_id, name, token_hash, created, last_seen) VALUES (?, ?, ?, ?, ?, ?)",
  )
    .bind(deviceId, account.id, name, await sha256(secret), now, now)
    .run();
  return json({ token: secret, account_id: account.id, device_id: deviceId, email: account.email });
}

async function listDevices(s: Session, env: Env): Promise<Response> {
  const { results } = await env.DB.prepare(
    "SELECT id, name, created, last_seen FROM devices WHERE account_id = ? ORDER BY created",
  )
    .bind(s.account_id)
    .all();
  return json({
    current: s.device_id,
    devices: results.map((d: any) => ({
      id: d.id,
      name: d.name,
      created: d.created,
      last_seen: d.last_seen,
    })),
  });
}

async function removeDevice(s: Session, env: Env, id: string): Promise<Response> {
  const done = await env.DB.prepare("DELETE FROM devices WHERE id = ? AND account_id = ?")
    .bind(id, s.account_id)
    .run();
  if (!done.meta.changes) throw new Fail(404, "no such device");
  return json({ ok: true });
}

// GET /v1/settings?since=<ms>  ->  changed entries (deleted ones have value null)
async function readSettings(s: Session, env: Env, url: URL): Promise<Response> {
  const since = Number(url.searchParams.get("since") ?? 0) || 0;
  const { results } = await env.DB.prepare(
    "SELECT key, value, updated FROM settings WHERE account_id = ? AND updated > ? ORDER BY updated",
  )
    .bind(s.account_id, since)
    .all();
  return json({ now: Date.now(), settings: results });
}

// PUT /v1/settings {changes: [{key, value|null, updated}]}  -> the stored state
// of each key sent. A change applies only if it is newer than what's stored.
async function writeSettings(s: Session, env: Env, req: Request): Promise<Response> {
  const input = await body(req);
  const changes = Array.isArray(input.changes) ? input.changes : [];
  if (changes.length > MAX_CHANGES) throw new Fail(413, "too many changes at once");
  const now = Date.now();

  const known = await env.DB.prepare("SELECT COUNT(*) AS n FROM settings WHERE account_id = ?")
    .bind(s.account_id)
    .first();
  let room = MAX_KEYS - known.n;

  const writes = [];
  const keys: string[] = [];
  for (const change of changes) {
    const key = String(change?.key ?? "");
    if (!key || key.length > MAX_KEY) throw new Fail(400, "bad key");
    const value = change.value === null || change.value === undefined ? null : String(change.value);
    if (value !== null && value.length > MAX_VALUE) throw new Fail(413, "value too large");
    const updated = Math.min(Number(change.updated) || now, now + 5 * 60_000);
    if (value !== null && room-- <= 0) throw new Fail(413, "too many settings");
    keys.push(key);
    writes.push(
      env.DB.prepare(
        `INSERT INTO settings (account_id, key, value, updated, device_id) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(account_id, key) DO UPDATE SET value = excluded.value,
           updated = excluded.updated, device_id = excluded.device_id
         WHERE excluded.updated > settings.updated`,
      ).bind(s.account_id, key, value, updated, s.device_id),
    );
  }
  if (writes.length) await env.DB.batch(writes);

  const stored = [];
  for (const key of new Set(keys)) {
    const row = await env.DB.prepare(
      "SELECT key, value, updated FROM settings WHERE account_id = ? AND key = ?",
    )
      .bind(s.account_id, key)
      .first();
    if (row) stored.push(row);
  }
  return json({ now, settings: stored });
}

async function route(req: Request, env: Env): Promise<Response> {
  const url = new URL(req.url);
  const path = url.pathname.replace(/\/+$/, "") || "/";
  const method = req.method;

  if (path === "/v1/config" && method === "GET") {
    return json({ host: env.BASE_HOST, api: "v1" });
  }
  if (path === "/v1/auth/code" && method === "POST") return requestCode(req, env);
  if (path === "/v1/auth/verify" && method === "POST") return verifyCode(req, env);
  if (path === "/v1/pair/claim" && method === "POST") return claimPairing(req, env);

  if (path.startsWith("/v1/")) {
    const s = await session(req, env);
    if (path === "/v1/me" && method === "GET") {
      return json({ account_id: s.account_id, device_id: s.device_id, email: s.email });
    }
    if (path === "/v1/auth/logout" && method === "POST") return removeDevice(s, env, s.device_id);
    if (path === "/v1/pair/start" && method === "POST") return startPairing(s, env);
    if (path === "/v1/devices" && method === "GET") return listDevices(s, env);
    if (path.startsWith("/v1/devices/") && method === "DELETE") {
      return removeDevice(s, env, path.slice("/v1/devices/".length));
    }
    if (path === "/v1/settings" && method === "GET") return readSettings(s, env, url);
    if (path === "/v1/settings" && method === "PUT") return writeSettings(s, env, req);
  }
  throw new Fail(404, "not found");
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    try {
      return await route(req, env);
    } catch (e) {
      if (e instanceof Fail) return json({ error: e.message }, e.status);
      console.error("unhandled:", e);
      return json({ error: "something went wrong" }, 500);
    }
  },
};
