"""A pretend Chromecast with a screen, on this computer, for testing Tonearm's
cast without a TV. Windows only.

Advertises itself through Windows' own mDNS (DnsServiceRegister), speaks
the Cast protocol over TLS on 8009, and shows Tonearm's real receiver page
(../public/index.html, read fresh on each load) in an Edge window, with
shim.js standing in for Google's receiver SDK. The window plays the stream.
Every message Tonearm sends is logged, lyrics with how many lines carry a
translation.

    python receiver/fake-cast/fake_cast.py [--name "Laptop TV (test)"] [--no-browser]

Phones and other computers reach it on port 8009, so Windows Firewall must
let Python in on the network's profile. Needs Git for Windows' openssl, which
makes a self-signed certificate on the first run. shot.ps1 saves the window
as tv.png.
"""

import argparse
import asyncio
import ctypes
import ctypes.wintypes as wt
import json
import os
import pathlib
import socket
import ssl
import subprocess
import sys
import time
import uuid

HERE = pathlib.Path(__file__).resolve().parent
RECEIVER_PAGE = HERE.parent / "public" / "index.html"
SDK_TAG = '<script src="https://www.gstatic.com/cast/sdk/libs/caf_receiver/v3/cast_receiver_framework.js"></script>'

CONNECTION = "urn:x-cast:com.google.cast.tp.connection"
HEARTBEAT = "urn:x-cast:com.google.cast.tp.heartbeat"
RECEIVER = "urn:x-cast:com.google.cast.receiver"
MEDIA = "urn:x-cast:com.google.cast.media"
TONEARM = "urn:x-cast:app.tonearm"

CAST_PORT = 8009
PAGE_PORT = 8010


def log(*parts):
    print(time.strftime("%H:%M:%S"), *parts, flush=True)


# ------------------------------------------------------------ the envelope

def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n == 0:
            out.append(b)
            return bytes(out)
        out.append(b | 0x80)


def text_field(field, text):
    data = text.encode()
    return varint(field << 3 | 2) + varint(len(data)) + data


def encode(source, destination, namespace, payload):
    body = (varint(1 << 3) + varint(0) + text_field(2, source) + text_field(3, destination)
            + text_field(4, namespace) + varint(5 << 3) + varint(0) + text_field(6, json.dumps(payload)))
    return len(body).to_bytes(4, "big") + body


def read_varint(data, at):
    n = shift = 0
    while True:
        b = data[at]
        at += 1
        n |= (b & 0x7F) << shift
        if not b & 0x80:
            return n, at
        shift += 7


def decode(data):
    fields = {}
    at = 0
    while at < len(data):
        key, at = read_varint(data, at)
        if key & 7 == 0:
            _, at = read_varint(data, at)
        elif key & 7 == 2:
            length, at = read_varint(data, at)
            fields[key >> 3] = data[at:at + length]
            at += length
        else:
            return None
    try:
        payload = json.loads(fields.get(6, b"null"))
    except ValueError:
        return None
    return fields.get(2, b"").decode(), fields.get(3, b"").decode(), fields.get(4, b"").decode(), payload


# ------------------------------------------------------------ the device

class Device:
    def __init__(self):
        self.volume = 0.3
        self.muted = False
        self.app = None  # {"appId", "sessionId", "transportId"}
        self.media_session = 0
        self.media_id = None
        self.page = {"state": "IDLE", "currentTime": 0.0}
        self.page_at = time.monotonic()
        self.wanted = "IDLE"
        self.announced = False
        self.loader = None  # (writer, sender id) that loaded the media
        self.pages = []  # queues of browser windows listening
        # What a window opened or reloaded mid-cast needs to catch up.
        self.last_load = None
        self.last_lyrics = {}
        self.last_now = None

    def to_pages(self, kind, **data):
        for q in self.pages:
            q.put_nowait({"kind": kind, **data})

    def receiver_status(self, request_id):
        apps = []
        if self.app:
            apps.append({
                **self.app,
                "displayName": "Tonearm",
                "statusText": "Tonearm",
                "isIdleScreen": False,
                "namespaces": [{"name": n} for n in (CONNECTION, MEDIA, TONEARM)],
            })
        return {
            "type": "RECEIVER_STATUS",
            "requestId": request_id,
            "status": {
                "applications": apps,
                "volume": {"level": self.volume, "muted": self.muted, "controlType": "attenuation", "stepInterval": 0.05},
            },
        }

    def media_status(self, request_id, state=None):
        if not self.media_id:
            return {"type": "MEDIA_STATUS", "requestId": request_id, "status": []}
        page_state = self.page["state"]
        if state is None:
            state = "IDLE" if page_state == "ERROR" else page_state
            if state == "IDLE" and self.wanted != "IDLE":
                state = "BUFFERING"
        # The window reports every half second. A real device's position is
        # fresh, so it is carried on to now while playing: a stale one would
        # read as the device falling behind, and Tonearm would send less.
        position = self.page.get("currentTime", 0.0)
        if page_state == "PLAYING":
            position += time.monotonic() - self.page_at
        status = {
            "mediaSessionId": self.media_id,
            "playbackRate": 1,
            "playerState": state,
            "currentTime": position,
            "supportedMediaCommands": 1 | 4 | 8 | 64 | 128,
            "volume": {"level": 1, "muted": False},
        }
        if page_state == "ERROR":
            status["playerState"] = "IDLE"
            status["idleReason"] = "ERROR"
        return {"type": "MEDIA_STATUS", "requestId": request_id, "status": [status]}


DEVICE = Device()


def summarise_lyrics(p):
    lines = p.get("lines", [])
    translated = [l for l in lines if l.get("translation")]
    sample = next((l for l in translated if l.get("text")), None)
    text = f" e.g. {sample['text']!r} -> {sample['translation']!r}" if sample else ""
    return f"LYRICS id={p.get('id')} lines={len(lines)} translated={len(translated)}{text}"


async def cast_connection(reader, writer):
    peer = writer.get_extra_info("peername")
    log("cast: connected from", peer)

    def send(source, destination, namespace, payload):
        writer.write(encode(source, destination, namespace, payload))

    try:
        while True:
            head = await reader.readexactly(4)
            body = await reader.readexactly(int.from_bytes(head, "big"))
            message = decode(body)
            if not message:
                continue
            source, destination, namespace, p = message
            kind = p.get("type") if isinstance(p, dict) else None
            rid = p.get("requestId", 0) if isinstance(p, dict) else 0

            if kind not in (None, "PING", "PONG", "GET_STATUS", "CONNECT", "LAUNCH", "LOAD") and namespace != TONEARM:
                log(f"cast: {kind} from {source}", json.dumps({k: v for k, v in p.items() if k not in ("type", "requestId")}))
            if namespace == HEARTBEAT:
                if kind == "PING":
                    send(destination, source, HEARTBEAT, {"type": "PONG"})
            elif namespace == CONNECTION:
                pass
            elif namespace == RECEIVER:
                if kind == "GET_STATUS":
                    send(destination, source, RECEIVER, DEVICE.receiver_status(rid))
                elif kind == "LAUNCH":
                    log("cast: LAUNCH", p.get("appId"))
                    DEVICE.to_pages("stop")
                    DEVICE.app = {
                        "appId": p.get("appId"),
                        "sessionId": str(uuid.uuid4()),
                        "transportId": "web-" + uuid.uuid4().hex[:8],
                    }
                    DEVICE.media_id = None
                    DEVICE.wanted = "IDLE"
                    DEVICE.last_load = DEVICE.last_now = None
                    DEVICE.last_lyrics = {}
                    send(destination, source, RECEIVER, DEVICE.receiver_status(rid))
                elif kind == "STOP":
                    log("cast: STOP")
                    DEVICE.app = None
                    DEVICE.media_id = None
                    DEVICE.wanted = "IDLE"
                    DEVICE.to_pages("stop")
                    send(destination, source, RECEIVER, DEVICE.receiver_status(rid))
                elif kind == "SET_VOLUME":
                    v = p.get("volume", {})
                    if "level" in v:
                        DEVICE.volume = float(v["level"])
                    if "muted" in v:
                        DEVICE.muted = bool(v["muted"])
                    DEVICE.to_pages("volume", level=DEVICE.volume, muted=DEVICE.muted)
                    send(destination, source, RECEIVER, DEVICE.receiver_status(rid))
            elif namespace == MEDIA:
                if kind == "LOAD":
                    DEVICE.media_session += 1
                    DEVICE.media_id = DEVICE.media_session
                    DEVICE.wanted = "PLAYING"
                    DEVICE.announced = False
                    DEVICE.page = {"state": "IDLE", "currentTime": 0.0}
                    DEVICE.loader = (writer, source)
                    DEVICE.last_load = p
                    log("cast: LOAD", p.get("media", {}).get("contentUrl"))
                    DEVICE.to_pages("volume", level=DEVICE.volume, muted=DEVICE.muted)
                    DEVICE.to_pages("load", request=p)
                    send(destination, source, MEDIA, DEVICE.media_status(rid, "BUFFERING"))
                elif kind == "GET_STATUS":
                    send(destination, source, MEDIA, DEVICE.media_status(rid))
                elif kind in ("PAUSE", "PLAY"):
                    DEVICE.wanted = "PAUSED" if kind == "PAUSE" else "PLAYING"
                    DEVICE.to_pages("pause" if kind == "PAUSE" else "play")
                    send(destination, source, MEDIA, DEVICE.media_status(rid, DEVICE.wanted))
                elif kind == "STOP":
                    DEVICE.wanted = "IDLE"
                    DEVICE.to_pages("stop")
            elif namespace == TONEARM:
                if kind == "LYRICS":
                    log("tonearm:", summarise_lyrics(p))
                    DEVICE.last_lyrics.pop(p.get("id"), None)
                    DEVICE.last_lyrics[p.get("id")] = p
                    while len(DEVICE.last_lyrics) > 4:
                        DEVICE.last_lyrics.pop(next(iter(DEVICE.last_lyrics)))
                elif kind == "NOW":
                    DEVICE.last_now = p
                if kind == "NOW" and p.get("id") != DEVICE.__dict__.get("now_id"):
                    DEVICE.now_id = p.get("id")
                    log(f"tonearm: NOW {p.get('title')!r} by {p.get('artist')!r}")
                DEVICE.to_pages("custom", namespace=TONEARM, data=p)
            await writer.drain()
    except (asyncio.IncompleteReadError, ConnectionError, ssl.SSLError) as e:
        log("cast: disconnected", peer, type(e).__name__)
    finally:
        if DEVICE.loader and DEVICE.loader[0] is writer:
            DEVICE.loader = None
        writer.close()


def page_status(status):
    if status.get("state") != DEVICE.page.get("state"):
        log(f"page: {DEVICE.page.get('state')} -> {status.get('state')} at {status.get('currentTime', 0):.2f}s")
    DEVICE.page = status
    DEVICE.page_at = time.monotonic()
    if status.get("state") == "ERROR":
        log("page: the audio element failed, code", status.get("error"))
    loader = DEVICE.loader
    if not loader or not DEVICE.app or not DEVICE.media_id:
        return
    writer, sender = loader
    if status.get("state") == "PLAYING" and not DEVICE.announced:
        DEVICE.announced = True
        log("page: playing")
        writer.write(encode(DEVICE.app["transportId"], sender, MEDIA, DEVICE.media_status(0, "PLAYING")))
    elif status.get("state") == "ERROR" and DEVICE.wanted != "IDLE":
        DEVICE.wanted = "IDLE"
        writer.write(encode(DEVICE.app["transportId"], sender, MEDIA, DEVICE.media_status(0)))


def page_custom(message):
    loader = DEVICE.loader
    if loader and DEVICE.app:
        writer, sender = loader
        log("page: custom", message.get("data"))
        writer.write(encode(DEVICE.app["transportId"], sender, message.get("namespace", TONEARM), message.get("data")))


# ------------------------------------------------------------ the page

def receiver_html():
    html = RECEIVER_PAGE.read_text(encoding="utf-8")
    if SDK_TAG not in html:
        log("page: warning, the Cast SDK tag was not found to replace")
    return html.replace(SDK_TAG, '<script src="/shim.js"></script>')


async def http_connection(reader, writer):
    try:
        head = await reader.readuntil(b"\r\n\r\n")
        lines = head.decode("latin-1").split("\r\n")
        method, path, _ = lines[0].split(" ", 2)
        headers = {k.lower(): v.strip() for k, v in (l.split(":", 1) for l in lines[1:] if ":" in l)}
        body = b""
        if "content-length" in headers:
            body = await reader.readexactly(int(headers["content-length"]))

        def respond(status, kind, data):
            writer.write(f"HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {len(data)}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n".encode() + data)

        if method == "GET" and path == "/":
            respond("200 OK", "text/html; charset=utf-8", receiver_html().encode())
        elif method == "GET" and path == "/shim.js":
            respond("200 OK", "text/javascript", (HERE / "shim.js").read_bytes())
        elif method == "GET" and path == "/events":
            writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\r\n")
            q = asyncio.Queue()
            DEVICE.pages.append(q)
            log("page: window connected")
            # A window opened or reloaded mid-cast catches up: it fetches the
            # stream again, as a device that lost it would.
            if DEVICE.app and DEVICE.media_id:
                q.put_nowait({"kind": "volume", "level": DEVICE.volume, "muted": DEVICE.muted})
                if DEVICE.last_load and DEVICE.wanted != "IDLE":
                    q.put_nowait({"kind": "load", "request": DEVICE.last_load})
                for lyrics in DEVICE.last_lyrics.values():
                    q.put_nowait({"kind": "custom", "namespace": TONEARM, "data": lyrics})
                if DEVICE.last_now:
                    q.put_nowait({"kind": "custom", "namespace": TONEARM, "data": DEVICE.last_now})
                if DEVICE.wanted == "PAUSED":
                    q.put_nowait({"kind": "pause"})
            try:
                while True:
                    try:
                        event = await asyncio.wait_for(q.get(), 15)
                        writer.write(b"data: " + json.dumps(event).encode() + b"\n\n")
                    except asyncio.TimeoutError:
                        writer.write(b": keepalive\n\n")
                    await writer.drain()
            finally:
                DEVICE.pages.remove(q)
                log("page: window gone")
        elif method == "POST" and path == "/status":
            page_status(json.loads(body or b"{}"))
            respond("204 No Content", "text/plain", b"")
        elif method == "POST" and path == "/event":
            e = json.loads(body or b"{}")
            log("audio:", " ".join(f"{k}={v}" for k, v in e.items() if v is not None))
            respond("204 No Content", "text/plain", b"")
        elif method == "POST" and path == "/custom":
            page_custom(json.loads(body or b"{}"))
            respond("204 No Content", "text/plain", b"")
        else:
            respond("404 Not Found", "text/plain", b"not found")
        await writer.drain()
    except (asyncio.IncompleteReadError, ConnectionError, ValueError):
        pass
    finally:
        writer.close()


# ------------------------------------------------------------ mDNS

class REGISTER_REQUEST(ctypes.Structure):
    _fields_ = [
        ("Version", wt.ULONG),
        ("InterfaceIndex", wt.ULONG),
        ("pServiceInstance", ctypes.c_void_p),
        ("pRegisterCompletionCallback", ctypes.c_void_p),
        ("pQueryContext", ctypes.c_void_p),
        ("hCredentials", wt.HANDLE),
        ("unicastEnabled", wt.BOOL),
    ]


class SERVICE_CANCEL(ctypes.Structure):
    _fields_ = [("reserved", ctypes.c_void_p)]


COMPLETE = ctypes.WINFUNCTYPE(None, wt.DWORD, ctypes.c_void_p, ctypes.c_void_p)


@COMPLETE
def registered(status, context, instance):
    log("mdns: registered" if status == 0 else f"mdns: registration failed ({status})")


def advertise(name, device_id, ip):
    dnsapi = ctypes.WinDLL("dnsapi")
    construct = dnsapi.DnsServiceConstructInstance
    construct.restype = ctypes.c_void_p
    construct.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p,
                          wt.WORD, wt.WORD, wt.WORD, wt.DWORD,
                          ctypes.POINTER(ctypes.c_wchar_p), ctypes.POINTER(ctypes.c_wchar_p)]
    txt = {"id": device_id, "cd": device_id.upper(), "rm": "", "ve": "05", "md": "Chromecast",
           "ic": "/setup/icon.png", "fn": name, "ca": "4101", "st": "0", "bs": "FA8FCA000000", "nf": "1", "rs": ""}
    keys = (ctypes.c_wchar_p * len(txt))(*txt.keys())
    values = (ctypes.c_wchar_p * len(txt))(*txt.values())
    ip4 = ctypes.c_uint32(int.from_bytes(socket.inet_aton(ip), "little"))
    host = os.environ.get("COMPUTERNAME", "laptop").lower() + ".local"
    instance = construct(f"Chromecast-{device_id}._googlecast._tcp.local", host, ctypes.byref(ip4), None,
                         CAST_PORT, 0, 0, len(txt), keys, values)
    if not instance:
        raise RuntimeError("DnsServiceConstructInstance failed")
    request = REGISTER_REQUEST(1, 0, instance, ctypes.cast(registered, ctypes.c_void_p), None, None, False)
    cancel = SERVICE_CANCEL()
    result = dnsapi.DnsServiceRegister(ctypes.byref(request), ctypes.byref(cancel))
    if result != 9506:  # DNS_REQUEST_PENDING
        raise RuntimeError(f"DnsServiceRegister returned {result}")
    log(f"mdns: advertising {name!r} at {ip}:{CAST_PORT} ({host})")
    # Kept for deregistering at exit.
    return dnsapi, request, keys, values, ip4


def deadvertise(handle):
    dnsapi, request, *_ = handle
    dnsapi.DnsServiceDeRegister(ctypes.byref(request), None)


# ------------------------------------------------------------ start

def local_ip():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        # Sends nothing: only picks the interface a LAN address would use.
        s.connect(("8.8.8.8", 9))
        return s.getsockname()[0]
    finally:
        s.close()


def certificate():
    cert, key = HERE / "cert.pem", HERE / "key.pem"
    if not cert.exists():
        openssl = r"C:\Program Files\Git\usr\bin\openssl.exe"
        subprocess.run([openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(key), "-out", str(cert),
                        "-days", "365", "-subj", "/CN=fake-cast"], check=True, capture_output=True)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(cert, key)
    return ctx


def open_window():
    edge = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
    profile = HERE / "edge-tv"
    subprocess.Popen([edge, f"--user-data-dir={profile}", f"--app=http://127.0.0.1:{PAGE_PORT}/",
                      "--window-size=1280,760", "--autoplay-policy=no-user-gesture-required",
                      "--no-first-run", "--no-default-browser-check"])


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--name", default="Laptop TV (test)")
    parser.add_argument("--no-browser", action="store_true")
    args = parser.parse_args()

    id_file = HERE / "device-id.txt"
    if not id_file.exists():
        id_file.write_text(uuid.uuid4().hex)
    device_id = id_file.read_text().strip()

    ip = local_ip()
    cast_server = await asyncio.start_server(cast_connection, "0.0.0.0", CAST_PORT, ssl=certificate())
    page_server = await asyncio.start_server(http_connection, "127.0.0.1", PAGE_PORT)
    log(f"cast: listening on {ip}:{CAST_PORT}, page on http://127.0.0.1:{PAGE_PORT}/")
    handle = advertise(args.name, device_id, ip)
    if not args.no_browser:
        open_window()
    try:
        async with cast_server, page_server:
            await asyncio.gather(cast_server.serve_forever(), page_server.serve_forever())
    finally:
        deadvertise(handle)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        sys.exit(0)
