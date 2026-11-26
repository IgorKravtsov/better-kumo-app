(() => {
  if (window.__kumo && window.__kumo.v === 10) return true;
  const findStore = () => {
    const root = document.getElementById("root") || document.body.firstElementChild;
    if (!root) return null;
    const key = Object.keys(root).find((k) => k.startsWith("__reactContainer$"));
    if (!key) return null;
    const stack = [root[key]];
    while (stack.length) {
      const n = stack.pop();
      if (!n) continue;
      const p = n.memoizedProps;
      if (p && p.store && typeof p.store.getState === "function") return p.store;
      if (n.child) stack.push(n.child);
      if (n.sibling) stack.push(n.sibling);
    }
    return null;
  };
  window.__kumo = {
    v: 10,
    // A reinjected script must keep a handed-off call's credentials: the left Daily call no longer has them.
    creds: (window.__kumo && window.__kumo.creds) || null,
    store: null,
    ws: null,
    s() {
      if (!this.store) this.store = findStore();
      return this.store ? this.store.getState() : null;
    },
    state() {
      const s = this.s();
      if (!s) return null;
      const self = s.users.userSelf;
      const room = (s.space.roomConfig && s.space.roomConfig.room) || {};
      const status = (u) => (u && u.userStatus && u.userStatus.availability) || "";
      const floors = ((s.space.spaceConfig && s.space.spaceConfig.rooms) || []).map((r) => ({ id: r.id, name: r.displayName || "" }));
      const heard = this.heard();
      const handedOff = !!(s.daily && s.daily.call && s.daily.call.__kumoHandoff);
      const call = this.call();
      const people = Object.values(s.users.usersMap || {}).filter((u) => u && u.type === "user");
      const ids = new Set(people.map((u) => u.id));
      return {
        floorId: s.space.roomName || (self && self.roomName) || null,
        width: room.width || 0,
        height: room.height || 0,
        floors,
        self: self ? { id: self.id, x: self.x, y: self.y, zoneId: self.zoneId || null, availability: status(self), message: (self.userStatus && self.userStatus.message) || "" } : null,
        zones: Object.values(s.space.zones || {}).filter((z) => z && z.width && z.height).map((z) => ({
          id: z.id, name: z.displayName || "", x: z.x, y: z.y, w: z.width, h: z.height, open: z.status !== "closed",
        })),
        people: people.map((u) => ({
          id: u.id, uid: u.firebaseUid || "", name: u.name || "", avatar: u.avatarUrl || "", x: u.x || 0, y: u.y || 0,
          zoneId: u.zoneId || null, self: !!u.self, availability: status(u),
          idle: !!(u.userStatus && u.userStatus.setBy === "idle"),
          message: (u.userStatus && u.userStatus.message) || "",
        })),
        audio: !!(s.daily.mediaEnabledState && s.daily.mediaEnabledState.audio),
        video: !!(s.daily.mediaEnabledState && s.daily.mediaEnabledState.video),
        earshot: heard ? heard.people.map((p) => p.id).filter((id) => ids.has(id)) : [],
        inCall: !!heard,
        call: (heard || handedOff) && call ? { roomName: call.roomName, peers: call.peers } : null,
      };
    },
    // kumo-meetings' page script reads this too, so recording and Desktop state see one Earshot.
    heard() {
      const s = this.s();
      const call = s && s.daily && s.daily.call;
      const state = call && call.meetingState ? call.meetingState() : "none";
      if (state !== "joined-meeting") return null;
      const parts = call.participants();
      const audio = (p) => (p && p.tracks && p.tracks.audio) || null;
      const person = (p) => ({ id: String(p.user_id || p.session_id), name: p.user_name || "", participant: p });
      return {
        me: parts.local ? person(parts.local) : null,
        people: Object.values(parts).filter((p) => p && !p.local && audio(p) && audio(p).subscribed === true).map(person),
      };
    },
    census() {
      const s = this.s();
      if (!s) return null;
      const zones = {};
      for (const z of Object.values(s.space.zones || {})) if (z && z.id) zones[z.id] = z.displayName || "";
      return {
        census: s.users.censusMap || {},
        floors: ((s.space.spaceConfig && s.space.spaceConfig.rooms) || []).map((r) => ({ id: r.id, name: r.displayName || "" })),
        zones,
      };
    },
    nearby() {
      const s = this.s();
      const me = s && s.users.censusSelf;
      const self = s && s.users.userSelf;
      if (!me || !self) return null;
      const range = s.space.roomConfig && s.space.roomConfig.room && s.space.roomConfig.room.audioRange;
      const near = (u) => self.zoneId || u.zoneId
        ? self.zoneId === u.zoneId
        : range === "room" || (self.x - u.x) ** 2 + (self.y - u.y) ** 2 <= range ** 2;
      const census = s.users.censusMap || {};
      const participantIds = range
        ? (s.users.users || []).filter(near).map((u) => census[u.id] && census[u.id].firebaseUid).filter(Boolean)
        : [];
      const zoneId = ((s.users.usersMap || {})[me.id] || {}).zoneId || null;
      const zone = zoneId && (s.space.zones || {})[zoneId];
      const messages = ((s.chat.session && s.chat.session.messages) || [])
        .filter((m) => m && m.type === "chat" && m.range === "nearby")
        .map((m) => ({
          id: m.id || null, senderId: m.senderId || null, senderName: m.senderName || (census[m.senderId] && census[m.senderId].name) || null,
          message: m.message || null, zoneId: m.zoneId || null, at: m.timestamp ? new Date(m.timestamp).getTime() : null,
        }));
      const floorId = s.space.roomName || self.roomName || null;
      return { selfId: me.id, floorId, zoneId, zoneName: (zone && zone.displayName) || null, participantIds, messages };
    },
    sendNearby(frame, floorId) {
      const now = this.nearby();
      const data = JSON.parse(frame).data;
      const current = new Set(now ? now.participantIds : []);
      const same = current.size === data.participantIds.length && data.participantIds.every((id) => current.has(id));
      if (!now || now.floorId !== floorId || now.zoneId !== (data.zoneId || null) || !same) return "moved";
      return this.send(frame);
    },
    send(frame) {
      const ws = this.ws;
      if (!ws || ws.readyState !== 1) return false;
      ws.send(frame);
      return true;
    },
    media(audio, video) {
      const s = this.s();
      const call = s && s.daily && s.daily.call;
      if (!call) return false;
      if (audio !== null && audio !== undefined) call.setLocalAudio(audio);
      if (video !== null && video !== undefined) call.setLocalVideo(video);
      return true;
    },
    call() {
      const s = this.s();
      const call = s && s.daily && s.daily.call;
      const props = (call && call.__kumoHandoff && this.creds) || (call && call.properties);
      const self = s && s.users.userSelf;
      if (!props || !props.url || !props.token || !self) return null;
      const peers = Object.values(s.users.usersMap || {})
        .filter((u) => u && u.type === "user" && !u.self && self.zoneId && u.zoneId === self.zoneId)
        .map((u) => u.id);
      const zone = (s.space.zones || {})[self.zoneId];
      return { url: props.url, token: props.token, selfId: self.id, peers, roomName: (zone && zone.displayName) || "" };
    },
    handoff() {
      const s = this.s();
      const call = s && s.daily && s.daily.call;
      if (!call) return false;
      const info = this.call();
      if (info) this.creds = { url: info.url, token: info.token };
      if (!call.__kumoHandoff) {
        call.__kumoHandoff = true;
        call.join = (opts) => {
          if (opts && opts.url && opts.token) window.__kumo.creds = { url: opts.url, token: opts.token };
          return new Promise(() => {});
        };
      }
      const state = call.meetingState();
      if (state === "joined-meeting" || state === "joining-meeting") return call.leave().then(() => true);
      return true;
    },
    reclaim() {
      const s = this.s();
      const call = s && s.daily && s.daily.call;
      if (!call || !call.__kumoHandoff) return false;
      delete call.join;
      delete call.__kumoHandoff;
      this.creds = null;
      return true;
    },
    home() {
      const s = this.s();
      const self = s && s.users.userSelf;
      const homes = s && s.space.spaceConfig && s.space.spaceConfig.spawnHomes;
      const home = self && homes && homes[self.firebaseUid];
      if (!home) return null;
      return { roomId: home.roomId, x: home.x, y: home.y, currentFloor: s.space.roomName || self.roomName };
    },
    broadcastShown() {
      return [...document.querySelectorAll('strong')].some((e) => /^(Broadcasting|Presenting) now$/.test(e.textContent.trim()));
    },
    nameOf(id) {
      const s = this.s();
      if (!s) return null;
      const u = (s.users.censusMap || {})[id] || (s.users.usersMap || {})[id] ||
        Object.values(s.users.censusMap || {}).find((c) => c.firebaseUid === id);
      return u ? u.name : null;
    },
  };
  return true;
})()
