// Parties: a shared queue the host's device plays and guests add to. One
// Durable Object per Party (PartyRoom, named by its code) holds its members,
// the host's latest state and everyone's WebSocket. It passes messages
// along and keeps nothing after the Party ends. Plan: docs/party.md.

// The Workers runtime module; its types aren't installed in this folder.
// @ts-ignore
import { DurableObject } from "cloudflare:workers";

const PARTY_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const MAX_MEMBERS = 50;
const MAX_MESSAGE = 64 * 1024;
const MESSAGES_PER_10S = 50;
const HOST_GONE_MS = 10 * 60_000; // the Party ends when the host is away this long
const LIFETIME_MS = 12 * 3_600_000; // and after this, whatever happens
const JOINS_PER_MINUTE = 20;

/** Six letters and digits with no look-alikes, as `K7Q-M4P`. */
export function partyCode(): string {
  const limit = 256 - (256 % PARTY_ALPHABET.length);
  let out = "";
  while (out.length < 6) {
    for (const b of crypto.getRandomValues(new Uint8Array(16))) {
      if (b < limit && out.length < 6) out += PARTY_ALPHABET[b % PARTY_ALPHABET.length];
    }
  }
  return `${out.slice(0, 3)}-${out.slice(3)}`;
}

/** A code as typed, scanned or linked, in its `K7Q-M4P` form, or null. */
export function cleanPartyCode(value: string): string | null {
  const clean = value.toUpperCase().replace(/[^A-Z0-9]/g, "");
  if (clean.length !== 6 || [...clean].some((c) => !PARTY_ALPHABET.includes(c))) return null;
  return `${clean.slice(0, 3)}-${clean.slice(3)}`;
}

function secret(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function sha256(text: string): Promise<string> {
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
  return [...hash].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function cleanName(value: unknown, fallback: string): string {
  const name = String(value ?? "").replace(/\s+/g, " ").trim().slice(0, 40);
  return name || fallback;
}

interface Member {
  id: string;
  name: string;
  host: boolean;
  token_hash: string;
  /** Songs this member has added. */
  added: number;
  joined: number;
}

interface Party {
  code: string;
  name: string;
  account_id: string;
  created: number;
  /** Guests may play, pause, skip, seek and reorder, not only add. */
  others_control: boolean;
  /** The host's last `state`, passed to everyone who connects. */
  state: unknown;
  state_at: number;
}

const reply = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });

/** Commands a guest may send. `add` always; the rest only when allowed. */
const GUEST_COMMANDS = new Set(["play", "pause", "next", "previous", "seek", "move", "play_item"]);

export class PartyRoom extends DurableObject {
  // Set by the runtime.
  declare ctx: any;
  /** Messages each socket sent lately, for the rate limit. Lost on hibernation, which is fine. */
  private sent = new Map<any, number[]>();

  constructor(ctx: any, env: any) {
    super(ctx, env);
    // Keep-alive pings are answered without waking the room.
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  private async party(): Promise<Party | undefined> {
    return this.ctx.storage.get("party");
  }

  private async members(): Promise<Member[]> {
    const map: Map<string, Member> = await this.ctx.storage.list({ prefix: "member:" });
    return [...map.values()].sort((a, b) => Number(b.host) - Number(a.host) || a.joined - b.joined);
  }

  /** Who is connected: member ids from the sockets' tags. */
  private online(except?: any): Set<string> {
    const ids = new Set<string>();
    for (const ws of this.ctx.getWebSockets()) {
      if (ws !== except) for (const tag of this.ctx.getTags(ws)) ids.add(tag);
    }
    return ids;
  }

  /** Everyone in the Party; `leaving` is a socket closing that no longer counts as online. */
  private async memberList(leaving?: any) {
    const online = this.online(leaving);
    return (await this.members()).map((m) => ({
      id: m.id,
      name: m.name,
      host: m.host,
      added: m.added,
      online: online.has(m.id),
    }));
  }

  private send(ws: any, message: unknown) {
    try {
      ws.send(JSON.stringify({ ...(message as object), now: Date.now() }));
    } catch {}
  }

  private broadcast(message: unknown, except?: any) {
    for (const ws of this.ctx.getWebSockets()) if (ws !== except) this.send(ws, message);
  }

  private toMember(id: string, message: unknown) {
    for (const ws of this.ctx.getWebSockets(id)) this.send(ws, message);
  }

  private hostId(members: Member[]): string | undefined {
    return members.find((m) => m.host)?.id;
  }

  /** Ends the Party for everyone and forgets it. */
  private async end(reason: string) {
    for (const ws of this.ctx.getWebSockets()) {
      this.send(ws, { type: "ended", reason });
      try {
        ws.close(1000, "the Party ended");
      } catch {}
    }
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.deleteAll();
  }

  private async member(token: string): Promise<Member | undefined> {
    if (!token) return undefined;
    const hash = await sha256(token);
    return (await this.members()).find((m) => m.token_hash === hash);
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const op = url.pathname;
    const party = await this.party();

    // Start: only for a code nobody holds.
    if (op === "/create") {
      if (party) return reply({ error: "taken" }, 409);
      const input: any = await req.json();
      const now = Date.now();
      const token = secret();
      const host: Member = {
        id: crypto.randomUUID(),
        name: cleanName(input.host_name, "Host"),
        host: true,
        token_hash: await sha256(token),
        added: 0,
        joined: now,
      };
      const made: Party = {
        code: input.code,
        name: cleanName(input.name, `${host.name}'s Party`),
        account_id: input.account_id,
        created: now,
        others_control: true,
        state: null,
        state_at: 0,
      };
      await this.ctx.storage.put({ party: made, [`member:${host.id}`]: host });
      await this.ctx.storage.setAlarm(now + HOST_GONE_MS);
      return reply({ code: made.code, name: made.name, token, member_id: host.id });
    }

    if (!party) return reply({ error: "no such Party, or it has ended" }, 404);

    if (op === "/summary") {
      const members = await this.members();
      const host = members.find((m) => m.host);
      return reply({
        code: party.code,
        name: party.name,
        host: host?.name ?? "",
        members: members.length,
        others_control: party.others_control,
        state: party.state,
      });
    }

    if (op === "/join") {
      const input: any = await req.json();
      const members = await this.members();
      if (members.length >= MAX_MEMBERS) return reply({ error: "this Party is full" }, 403);
      const now = Date.now();
      const recent: number[] = ((await this.ctx.storage.get("joins")) ?? []).filter((t: number) => now - t < 60_000);
      if (recent.length >= JOINS_PER_MINUTE) return reply({ error: "too many people joining; wait a minute" }, 429);
      recent.push(now);
      const token = secret();
      const member: Member = {
        id: crypto.randomUUID(),
        name: cleanName(input.name, "Guest"),
        host: false,
        token_hash: await sha256(token),
        added: 0,
        joined: now,
      };
      await this.ctx.storage.put({ [`member:${member.id}`]: member, joins: recent });
      this.broadcast({ type: "members", members: await this.memberList() });
      return reply({ code: party.code, name: party.name, token, member_id: member.id });
    }

    const token = (req.headers.get("authorization") ?? "").replace(/^Bearer /, "").trim() || url.searchParams.get("token") || "";
    const me = await this.member(token);
    if (!me) return reply({ error: "not in this Party" }, 401);

    if (op === "/end") {
      if (!me.host) return reply({ error: "only the host can end the Party" }, 403);
      await this.end("ended by the host");
      return reply({ ok: true });
    }

    if (op === "/live") {
      if (req.headers.get("upgrade")?.toLowerCase() !== "websocket") return reply({ error: "WebSocket only" }, 426);
      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair) as any[];
      this.ctx.acceptWebSocket(server, [me.id]);
      if (me.host) await this.ctx.storage.setAlarm(Math.min(party.created + LIFETIME_MS, Date.now() + LIFETIME_MS));
      this.send(server, {
        type: "welcome",
        you: me.id,
        host: me.host,
        code: party.code,
        name: party.name,
        others_control: party.others_control,
        members: await this.memberList(),
        state: party.state,
        state_at: party.state_at,
      });
      this.broadcast({ type: "members", members: await this.memberList() }, server);
      return new Response(null, { status: 101, webSocket: client } as any);
    }

    return reply({ error: "not found" }, 404);
  }

  async webSocketMessage(ws: any, raw: string | ArrayBuffer) {
    if (typeof raw !== "string" || raw.length > MAX_MESSAGE) {
      this.send(ws, { type: "error", error: "message too large" });
      return;
    }
    const now = Date.now();
    const times = (this.sent.get(ws) ?? []).filter((t) => now - t < 10_000);
    if (times.length >= MESSAGES_PER_10S) {
      this.send(ws, { type: "error", error: "slow down" });
      return;
    }
    times.push(now);
    this.sent.set(ws, times);

    let message: any;
    try {
      message = JSON.parse(raw);
    } catch {
      this.send(ws, { type: "error", error: "send JSON" });
      return;
    }
    const party = await this.party();
    if (!party) return;
    const [id] = this.ctx.getTags(ws);
    const members = await this.members();
    const me = members.find((m) => m.id === id);
    if (!me) {
      // Removed while connected.
      try {
        ws.close(4001, "removed");
      } catch {}
      return;
    }
    const type = String(message?.type ?? "");

    if (me.host) {
      switch (type) {
        case "state":
          party.state = message.state ?? null;
          party.state_at = now;
          await this.ctx.storage.put("party", party);
          this.broadcast({ type: "state", state: party.state, state_at: now }, ws);
          return;
        case "settings":
          party.others_control = Boolean(message.others_control);
          await this.ctx.storage.put("party", party);
          this.broadcast({ type: "settings", others_control: party.others_control });
          return;
        case "remove": {
          const gone = members.find((m) => m.id === message.member && !m.host);
          if (!gone) {
            this.send(ws, { type: "error", error: "no such member" });
            return;
          }
          await this.ctx.storage.delete(`member:${gone.id}`);
          for (const socket of this.ctx.getWebSockets(gone.id)) {
            this.send(socket, { type: "removed" });
            try {
              socket.close(4001, "removed");
            } catch {}
          }
          this.broadcast({ type: "members", members: await this.memberList() });
          return;
        }
        case "added": {
          // The host counts a guest's song once it is in the queue.
          const who = members.find((m) => m.id === message.member);
          if (who) {
            who.added += 1;
            await this.ctx.storage.put(`member:${who.id}`, who);
            this.broadcast({ type: "members", members: await this.memberList() });
          }
          return;
        }
        case "end":
          await this.end("ended by the host");
          return;
      }
      this.send(ws, { type: "error", error: `unknown message ${type}` });
      return;
    }

    // A guest.
    if (type === "leave") {
      await this.ctx.storage.delete(`member:${me.id}`);
      try {
        ws.close(1000, "left");
      } catch {}
      this.broadcast({ type: "members", members: await this.memberList(ws) }, ws);
      return;
    }
    if (type === "add" || GUEST_COMMANDS.has(type)) {
      if (type !== "add" && !party.others_control) {
        this.send(ws, { type: "error", error: "the host only lets others add songs" });
        return;
      }
      const host = this.hostId(members);
      if (!host || !this.online().has(host)) {
        this.send(ws, { type: "error", error: "the host is away right now" });
        return;
      }
      const { type: _, ...args } = message;
      this.toMember(host, { type: "command", from: me.id, from_name: me.name, command: type, args });
      return;
    }
    this.send(ws, { type: "error", error: `unknown message ${type}` });
  }

  async webSocketClose(ws: any) {
    this.sent.delete(ws);
    const party = await this.party();
    if (!party) return;
    const [id] = this.ctx.getTags(ws);
    const members = await this.members();
    // The host leaving starts the clock: back within HOST_GONE_MS or the Party ends.
    if (id === this.hostId(members)) {
      const still = this.ctx.getWebSockets(id).filter((s: any) => s !== ws).length;
      if (!still) await this.ctx.storage.setAlarm(Math.min(Date.now() + HOST_GONE_MS, party.created + LIFETIME_MS));
    }
    this.broadcast({ type: "members", members: await this.memberList(ws) }, ws);
  }

  async webSocketError(ws: any) {
    await this.webSocketClose(ws);
  }

  async alarm() {
    const party = await this.party();
    if (!party) return;
    const now = Date.now();
    if (now >= party.created + LIFETIME_MS) {
      await this.end("the Party ran for 12 hours");
      return;
    }
    const host = this.hostId(await this.members());
    if (!host || !this.online().has(host)) {
      await this.end("the host was away too long");
      return;
    }
    // The host is here: check again at the end of the Party's life.
    await this.ctx.storage.setAlarm(party.created + LIFETIME_MS);
  }
}
