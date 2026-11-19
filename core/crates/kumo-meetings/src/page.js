(() => {
  if (window.__kumoMeet && window.__kumoMeet.v === 3) return true;
  if (window.__kumoMeet && window.__kumoMeet.stop) window.__kumoMeet.stop();
  const RATE = 16000;
  const WORKLET = `registerProcessor("kumo-pcm", class extends AudioWorkletProcessor {
    constructor() { super(); this.buf = new Int16Array(1600); this.n = 0; }
    process(inputs) {
      const ch = inputs[0] && inputs[0][0];
      if (ch) for (let i = 0; i < ch.length; i++) {
        const v = Math.max(-1, Math.min(1, ch[i]));
        this.buf[this.n++] = v < 0 ? v * 32768 : v * 32767;
        if (this.n === this.buf.length) {
          this.port.postMessage({ pcm: this.buf, frame: currentFrame + i + 1 - this.buf.length }, [this.buf.buffer]);
          this.buf = new Int16Array(1600);
          this.n = 0;
        }
      }
      return true;
    }
  });`;
  const base64 = (pcm) => {
    const bytes = new Uint8Array(pcm.buffer, pcm.byteOffset, pcm.byteLength);
    let s = "";
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s);
  };
  const store = () => (window.__kumo && window.__kumo.s ? window.__kumo.s() : null);
  window.__kumoMeet = {
    v: 3,
    on: false,
    seen: 0,
    epoch: 0,
    watchdog: null,
    ctx: null,
    loading: null,
    error: null,
    nodes: new Map(),
    out: [],
    earshot() {
      const s = store();
      const call = s && s.daily && s.daily.call;
      const state = call && call.meetingState ? call.meetingState() : "none";
      if (state !== "joined-meeting") return { inCall: false, me: null, people: [], zone: "", tracks: [] };
      const parts = call.participants();
      const audio = (p) => (p && p.tracks && p.tracks.audio) || null;
      const track = (p) => (audio(p) && (audio(p).persistentTrack || audio(p).track)) || null;
      const person = (p) => ({ id: String(p.user_id || p.session_id), name: p.user_name || "" });
      const heard = Object.values(parts).filter((p) => p && !p.local && audio(p) && audio(p).subscribed === true);
      const me = parts.local ? person(parts.local) : null;
      const self = s.users && s.users.userSelf;
      const zone = self && self.zoneId && s.space && s.space.zones && s.space.zones[self.zoneId];
      const tracks = [];
      if (me && track(parts.local)) tracks.push({ ...me, me: true, track: track(parts.local) });
      for (const p of heard) if (track(p)) tracks.push({ ...person(p), me: false, track: track(p) });
      return { inCall: true, me, people: heard.map(person), zone: (zone && zone.displayName) || "", tracks };
    },
    async open() {
      const ctx = new AudioContext({ sampleRate: RATE });
      await ctx.audioWorklet.addModule("data:application/javascript," + encodeURIComponent(WORKLET));
      if (!this.on) {
        ctx.close().catch(() => {});
        return;
      }
      this.ctx = ctx;
      this.watchdog = setInterval(() => {
        if (Date.now() - this.seen > 5000) this.stop();
      }, 1000);
    },
    attach(t) {
      const source = this.ctx.createMediaStreamSource(new MediaStream([t.track]));
      const node = new AudioWorkletNode(this.ctx, "kumo-pcm", { numberOfInputs: 1, numberOfOutputs: 0 });
      node.port.onmessage = (e) => {
        const { pcm, frame } = e.data;
        this.out.push({ id: t.id, name: t.name, me: t.me, at: Math.round(this.epoch + (frame * 1000) / RATE), pcm: base64(pcm) });
        if (this.out.length > 3000) this.out.splice(0, this.out.length - 3000);
      };
      source.connect(node);
      this.nodes.set(t.id, { trackId: t.track.id, source, node });
    },
    detach(id) {
      const n = this.nodes.get(id);
      if (!n) return;
      try {
        n.node.port.onmessage = null;
        n.source.disconnect();
        n.node.disconnect();
      } catch (e) {}
      this.nodes.delete(id);
    },
    stop() {
      this.on = false;
      for (const id of [...this.nodes.keys()]) this.detach(id);
      if (this.ctx) this.ctx.close().catch(() => {});
      this.ctx = null;
      clearInterval(this.watchdog);
      this.watchdog = null;
      this.out = [];
    },
    sync(capture, tracks) {
      if (!capture) {
        if (this.on || this.ctx) this.stop();
        return;
      }
      this.on = true;
      if (!this.ctx) {
        if (!this.loading) {
          this.loading = this.open()
            .catch((e) => { this.error = String(e); })
            .finally(() => { this.loading = null; });
        }
        return;
      }
      if (this.ctx.state === "suspended") this.ctx.resume().catch(() => {});
      this.epoch = Date.now() - this.ctx.currentTime * 1000;
      const want = new Map(tracks.map((t) => [t.id, t]));
      for (const [id, n] of [...this.nodes]) {
        const t = want.get(id);
        if (!t || t.track.id !== n.trackId || t.track.readyState === "ended") this.detach(id);
      }
      for (const t of tracks) {
        if (this.nodes.has(t.id) || t.track.readyState === "ended") continue;
        try {
          this.attach(t);
        } catch (e) {
          this.error = String(e);
        }
      }
    },
    tick(capture) {
      this.seen = Date.now();
      const e = this.earshot();
      this.sync(capture && e.inCall, e.tracks);
      const error = this.error;
      this.error = null;
      return { inCall: e.inCall, me: e.me, people: e.people, zone: e.zone, capturing: this.nodes.size, error, chunks: this.out.splice(0) };
    },
  };
  return true;
})()
