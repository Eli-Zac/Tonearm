// A stand-in for Google's Cast receiver framework, just enough for
// Tonearm's receiver page. Messages come from fake_cast.py over
// server-sent events and go back with fetch.
(function () {
  const post = (path, body) =>
    fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }).catch(() => {});

  class PlayerManager {
    constructor() {
      this.interceptors = {};
      this.listeners = {};
      this.media = null;
      this.el = null;
      this.loadedOnce = false;
      this.wanted = 'IDLE';
    }
    setMediaElement(el) {
      this.el = el;
      const report = () => this.report();
      for (const e of ['playing', 'pause', 'waiting', 'error', 'ended']) el.addEventListener(e, report);
      // Every step of the audio element's loading, for the log.
      const t0 = performance.now();
      for (const e of ['loadstart', 'loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'playing', 'pause',
        'waiting', 'stalled', 'suspend', 'emptied', 'abort', 'error', 'ended']) {
        el.addEventListener(e, () => {
          const b = el.buffered.length ? el.buffered.end(el.buffered.length - 1).toFixed(2) : '-';
          post('/event', {
            event: e,
            t: ((performance.now() - t0) / 1000).toFixed(2),
            readyState: el.readyState,
            networkState: el.networkState,
            currentTime: el.currentTime.toFixed(2),
            buffered: b,
            error: el.error ? el.error.code + ' ' + (el.error.message || '') : null,
          });
        });
      }
      el.addEventListener('playing', () => {
        if (!this.loadedOnce) {
          this.loadedOnce = true;
          (this.listeners.PLAYER_LOAD_COMPLETE || []).forEach((f) => f({}));
        }
      });
      setInterval(report, 500);
    }
    state() {
      const el = this.el;
      if (!el || !el.currentSrc) return 'IDLE';
      if (el.error) return 'ERROR';
      if (el.paused) return this.wanted === 'PLAYING' ? 'BUFFERING' : 'PAUSED';
      if (el.readyState < 3) return 'BUFFERING';
      return 'PLAYING';
    }
    report() {
      post('/status', { state: this.state(), currentTime: this.el ? this.el.currentTime : 0, error: this.el && this.el.error ? String(this.el.error.code) : null });
    }
    setMessageInterceptor(type, f) { this.interceptors[type] = f; }
    addEventListener(type, f) { (this.listeners[type] = this.listeners[type] || []).push(f); }
    getMediaInformation() { return this.media; }
    setMediaInformation(m) { this.media = m; }
    setSupportedMediaCommands() {}
    load(request) {
      const hook = this.interceptors.LOAD;
      const r = hook ? hook(request) || request : request;
      this.media = r.media;
      this.loadedOnce = false;
      this.wanted = 'PLAYING';
      this.el.src = r.media.contentUrl || r.media.contentId;
      this.el.play().catch((e) => post('/event', { event: 'play() rejected', error: String(e) }));
    }
  }

  class CastReceiverContext {
    constructor() {
      this.player = new PlayerManager();
      this.custom = {};
    }
    static getInstance() {
      if (!CastReceiverContext.one) CastReceiverContext.one = new CastReceiverContext();
      return CastReceiverContext.one;
    }
    getPlayerManager() { return this.player; }
    addCustomMessageListener(ns, f) { (this.custom[ns] = this.custom[ns] || []).push(f); }
    sendCustomMessage(ns, sender, data) { post('/custom', { namespace: ns, data }); }
    start() {
      const events = new EventSource('/events');
      events.onmessage = (e) => {
        const m = JSON.parse(e.data);
        const p = this.player;
        if (m.kind === 'load') p.load(m.request);
        else if (m.kind === 'pause') { p.wanted = 'PAUSED'; p.el.pause(); }
        else if (m.kind === 'play') { p.wanted = 'PLAYING'; p.el.play().catch(() => {}); }
        else if (m.kind === 'volume') { p.el.volume = Math.max(0, Math.min(1, m.level)); p.el.muted = !!m.muted; }
        else if (m.kind === 'stop') { p.wanted = 'IDLE'; p.el.pause(); p.el.removeAttribute('src'); p.el.load(); }
        else if (m.kind === 'custom') (this.custom[m.namespace] || []).forEach((f) => f({ data: m.data, senderId: 'sender-0' }));
      };
    }
  }

  class MusicTrackMediaMetadata { constructor() { this.metadataType = 3; } }
  class Image { constructor(url) { this.url = url; } }
  class CastReceiverOptions {}

  window.cast = {
    framework: {
      CastReceiverContext,
      CastReceiverOptions,
      messages: {
        MessageType: { LOAD: 'LOAD', MEDIA_STATUS: 'MEDIA_STATUS', QUEUE_UPDATE: 'QUEUE_UPDATE', QUEUE_NEXT: 'QUEUE_NEXT', QUEUE_PREV: 'QUEUE_PREV', SEEK: 'SEEK' },
        Command: { PAUSE: 1, SEEK: 2, STREAM_VOLUME: 4, STREAM_MUTE: 8, QUEUE_NEXT: 64, QUEUE_PREV: 128 },
        MusicTrackMediaMetadata,
        Image,
      },
      events: { EventType: { PLAYER_LOAD_COMPLETE: 'PLAYER_LOAD_COMPLETE' } },
      system: { MessageType: { JSON: 'JSON' } },
    },
  };
})();
