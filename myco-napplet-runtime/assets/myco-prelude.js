// Myco's supplement to the vendored `@napplet/shim` prelude.
//
// Trusted code, shipped in the APK, injected into every napplet's srcdoc
// after the vendored installer has run (see `src/prelude.rs`). It fills in
// what the vendored build does not carry:
//
// - `window.napplet.shell` — NAP-SHELL's napplet-side half. The vendored shim
//   has no `shell` domain, so without this `shell.supports()` — the spec's
//   discovery mechanism — is undefined for every domain.
// - `window.napplet.gamepad` — NAP-GAMEPAD cached controller snapshots.
// - `window.napplet.mesh` — NAP-MESH (`docs/design/napplet/NAP-MESH.md`),
//   Myco's own domain, which the vendored installer filters out because it is
//   not in the upstream registry yet.
// - `window.napplet.local` — NAP-LOCAL (`docs/design/napplet/NAP-LOCAL.md`),
//   Myco's keep-on-this-device domain, filtered out for the same reason; and
//   `window.napplet.resource.keep`, its blob half, added to the vendored
//   `resource` object.
// - `window.napplet.outbox.publish` — replaced, same wire and same result
//   shape, only to wait longer. The vendored one gives up after 30 s, and a
//   publish is signed first: with a signer app (NIP-55) that is a human
//   reading an approval screen, which routinely takes longer. See
//   `SIGNING_TIMEOUT_MS`.
// - `window.napplet.upload.upload` — replaced for the same reason
//   (NAP-UPLOAD, napplet/naps PR #33): the upload's
//   authorization is signed as the user before anything is sent.
// - `window.napplet.fs.pickFile` / `pickFiles` / `pickSaveFile` — replaced
//   for the same reason (NAP-FS, napplet/naps PR #88): the answer comes once
//   the user has chosen a file in the system picker, which can take a while.
// - `window.napplet.intent` — replaced whole (NAP-INTENT). The vendored build
//   has the first draft's `open(archetype, payload, options)`; the current
//   draft's binding takes a convention URI, `open("napplet:profile/open?…")`,
//   with an optional `#naddr1…` recommending a handler, and adds
//   `onDelivery`. Published napplets use both, so this takes both. It also
//   adds `launch(naddr)` (napplet/naps#148): open, or offer to install, one
//   named napplet.
//
// Same wire as everything else: flat NIP-5D envelopes posted to the parent
// frame, results correlated by `id`, pushes routed by `subId`. Nothing here is
// a boundary — dispatch on the Rust side refuses what was not granted, whatever
// this namespace contains.
//
// Defined as one global installer, `MycoPrelude.install({ domains })`, and
// invoked by the rendered prelude with the same domain list the vendored
// installer got.
var MycoPrelude = (function () {
  "use strict";

  var REQUEST_TIMEOUT_MS = 30000;

  // How long a request that is signed before it is answered may wait: the
  // publishes. The runtime bounds the signing itself (the signer app gets
  // 120 s, `myco-core` `external_signer::ANSWER_TIMEOUT`) and then the relay
  // write (seconds), and answers either way — with the event, or with
  // `ok: false` and why. This is only the backstop for a runtime that never
  // answers, so it sits well clear of that bound: a backstop that fires first
  // turns an approval the user is still giving into "timed out", and the
  // event may then be published anyway, behind the napplet's back.
  var SIGNING_TIMEOUT_MS = 300000;

  function install(options) {
    var domains = new Set((options && options.domains) || []);
    var napplet = window.napplet || (window.napplet = {});
    var routers = [];

    window.addEventListener("message", function (event) {
      if (event.source !== window.parent) return;
      var msg = event.data;
      if (typeof msg !== "object" || msg === null || typeof msg.type !== "string") return;
      for (var i = 0; i < routers.length; i++) routers[i](msg);
    });

    installShell(napplet, domains, routers);
    if (domains.has("gamepad")) installGamepad(napplet, routers);
    if (domains.has("outbox") && napplet.outbox) installOutboxPublish(napplet, routers);
    if (domains.has("mesh")) installMesh(napplet, routers);
    if (domains.has("local")) installLocal(napplet, routers);
    if (domains.has("resource") && napplet.resource) installResourceKeep(napplet, routers);
    if (domains.has("upload") && napplet.upload) installUploadUpload(napplet, routers);
    if (domains.has("fs") && napplet.fs) installFsPick(napplet, routers);
    if (domains.has("intent")) installIntent(napplet, routers);
    return napplet;
  }

  // --- NAP-GAMEPAD: cached snapshots, no requests on the render path -------
  function installGamepad(napplet, routers) {
    var state = { available: true, focused: false, pads: [] };
    var listeners = new Set();
    var subscribed = false;
    var ready = false;
    var nativeDescriptor = Object.getOwnPropertyDescriptor(Navigator.prototype, "getGamepads");
    var native = nativeDescriptor && nativeDescriptor.value;
    var replaceNative = false;
    if (typeof native === "function") {
      try { native.call(navigator); }
      catch (error) { replaceNative = error.name === "SecurityError"; }
    }
    var nativePads = [];
    function own(proto, values) {
      var result = Object.create(proto);
      Object.keys(values).forEach(function (key) {
        Object.defineProperty(result, key, { value: values[key], enumerable: true });
      });
      return result;
    }
    function nativePad(pad, connected) {
      return own(typeof Gamepad === "function" ? Gamepad.prototype : Object.prototype, {
        index: pad.index, id: pad.id, mapping: pad.mapping, connected: connected,
        timestamp: pad.timestamp, axes: Object.freeze(pad.axes.slice()),
        buttons: Object.freeze(pad.buttons.map(function (button) {
          return own(typeof GamepadButton === "function" ? GamepadButton.prototype : Object.prototype,
            { value: button.value, pressed: button.pressed, touched: button.touched });
        })),
        vibrationActuator: null, hapticActuators: Object.freeze([])
      });
    }
    function connectionEvent(type, pad) {
      // GamepadEvent's constructor requires a native controller, but Event has
      // the internal event slots needed by native dispatch and on* handlers.
      var event = new Event(type);
      if (typeof GamepadEvent === "function") Object.setPrototypeOf(event, GamepadEvent.prototype);
      Object.defineProperty(event, "gamepad", { value: pad, enumerable: true });
      window.dispatchEvent(event);
    }
    function postSubscribe() {
      window.parent.postMessage({ type: "gamepad.subscribe" }, "*");
    }
    // Startup use waits for the handshake, while reads always stay synchronous.
    napplet.shell.ready.then(function () {
      ready = true;
      if (subscribed) postSubscribe();
    });
    function ensureSubscription() {
      if (subscribed) return;
      subscribed = true;
      if (ready) postSubscribe();
    }
    function snapshot() {
      // Consumers cannot mutate the cached state seen by another consumer.
      return { available: state.available, focused: state.focused,
        ...(state.reason ? { reason: state.reason } : {}),
        pads: state.pads.map(function (pad) {
          return pad && { ...pad, axes: pad.axes.slice(),
            buttons: pad.buttons.map(function (button) { return { ...button }; }) };
        }) };
    }
    routers.push(function (msg) {
      if (msg.type !== "gamepad.state" || !Array.isArray(msg.pads)) return;
      var previous = nativePads;
      state = { available: msg.available === true, focused: msg.available === true && msg.focused === true,
        pads: msg.pads };
      if (!state.available) state.reason = msg.reason === "blocked" ? "blocked" : "unavailable";
      if (replaceNative) {
        nativePads = state.pads.map(function (pad) { return pad && nativePad(pad, true); });
        for (var i = 0; i < Math.max(previous.length, nativePads.length); i++) {
          var before = previous[i], after = nativePads[i];
          var same = before && after && before.id === after.id && before.mapping === after.mapping;
          if (before && !same) connectionEvent("gamepaddisconnected", nativePad(before, false));
          if (after && !same) connectionEvent("gamepadconnected", after);
        }
      }
      listeners.forEach(function (handler) {
        try { handler(snapshot()); } catch (error) { console.error("gamepad handler threw", error); }
      });
    });
    Object.defineProperty(napplet, "gamepad", { enumerable: true, value: Object.freeze({
      getGamepads: function () { ensureSubscription(); return snapshot().pads; },
      get available() { ensureSubscription(); return state.available; },
      get focused() { ensureSubscription(); return state.focused; },
      onChange: function (handler) {
        if (typeof handler !== "function") throw new TypeError("gamepad handler must be a function");
        listeners.add(handler);
        ensureSubscription();
        return { close: function () { listeners.delete(handler); } };
      }
    }) });
    if (replaceNative) {
      var replacement = {
        getGamepads() {
          // Match the native receiver check, including objects that merely
          // inherit Navigator.prototype, without throwing a native policy
          // exception on every animation frame.
          if (this !== navigator) throw new TypeError("Illegal invocation");
          ensureSubscription();
          if (state.reason === "blocked") throw new DOMException("Controller access denied", "SecurityError");
          return nativePads.slice();
        }
      }.getGamepads;
      Object.defineProperty(Navigator.prototype, "getGamepads", {
        ...nativeDescriptor, value: replacement
      });
      // Event-driven games may never poll until gamepadconnected fires.
      ensureSubscription();
    }
    window.addEventListener("pagehide", function () {
      if (subscribed) window.parent.postMessage({ type: "gamepad.unsubscribe" }, "*");
      subscribed = false;
    });
  }

  // --- NAP-SHELL: the napplet-side half ------------------------------------
  //
  // `supports()` and `services` answer from the environment `shell.init`
  // delivered. Until it arrives they answer from the injected list, which is
  // the same list the runtime will send — both come from one source — so a
  // napplet that checks at the top of its script gets the right answer rather
  // than a false "no" it would act on for good.
  function installShell(napplet, domains, routers) {
    var offered = domains;
    var services = Object.freeze([]);
    var readyResolve;
    var ready = new Promise(function (resolve) { readyResolve = resolve; });

    routers.push(function (msg) {
      if (msg.type !== "shell.init") return;
      var caps = msg.capabilities && Array.isArray(msg.capabilities.domains)
        ? msg.capabilities.domains.filter(function (d) { return typeof d === "string"; })
        : [];
      offered = new Set(caps);
      services = Object.freeze(Array.isArray(msg.services) ? msg.services.slice() : []);
      readyResolve({ domains: caps.slice(), services: services });
    });

    var shell = {
      supports: function (domain) {
        return typeof domain === "string" && offered.has(domain);
      },
      get services() { return services; },
      set services(_ignored) { /* runtime-owned */ },
      /** Resolves with the `shell.init` environment once it has arrived. */
      ready: ready
    };
    Object.defineProperty(napplet, "shell", { value: shell, enumerable: true, configurable: false, writable: false });
  }

  // --- NAP-OUTBOX: publish, with a signing-length wait ----------------------
  //
  // A drop-in for the vendored `outbox.publish`: the same envelope, resolved
  // with the same object built from the same fields — `ok`, `event`,
  // `eventId`, `relays`, `error` — so a caller cannot tell the two apart,
  // except that this one waits `SIGNING_TIMEOUT_MS` rather than 30 s.
  //
  // The vendored router still sees our `outbox.publish.result`s and drops
  // them: it resolves only ids in its own pending map, and ours never enter
  // it. Its other `outbox.*` calls are untouched.
  //
  // `relay.publish` needs no such fix: the vendored one sets no timer at all
  // and waits for its result however long signing takes.
  // The vendored shim's send (`cloneMode: "auto"`) retries a message the
  // structured clone refuses — a template held in a reactive Proxy, say — as a
  // plain snapshot. Kept here so the replacement publishes whatever the
  // original did. Event templates are JSON, so a JSON round trip is the snapshot.
  function postCloneable(message) {
    try {
      window.parent.postMessage(message, "*");
    } catch (e) {
      if (!e || e.name !== "DataCloneError") throw e;
      window.parent.postMessage(JSON.parse(JSON.stringify(message)), "*");
    }
  }

  function installOutboxPublish(napplet, routers) {
    var pending = new Map();

    routers.push(function (msg) {
      if (msg.type !== "outbox.publish.result" || typeof msg.id !== "string") return;
      var p = pending.get(msg.id);
      if (!p) return;
      pending.delete(msg.id);
      clearTimeout(p.timeout);
      var result = { ok: msg.ok };
      if (msg.event !== undefined) result.event = msg.event;
      if (msg.eventId !== undefined) result.eventId = msg.eventId;
      if (msg.relays !== undefined) result.relays = msg.relays;
      if (msg.error !== undefined) result.error = msg.error;
      p.resolve(result);
    });

    napplet.outbox.publish = function publish(template, options) {
      var id = crypto.randomUUID();
      return new Promise(function (resolve, reject) {
        var timeout = setTimeout(function () {
          if (pending.delete(id)) reject(new Error("outbox.publish timed out"));
        }, SIGNING_TIMEOUT_MS);
        pending.set(id, { resolve: resolve, timeout: timeout });
        var message = { type: "outbox.publish", id: id, event: template };
        if (options !== undefined) message.options = options;
        try {
          postCloneable(message);
        } catch (e) {
          clearTimeout(timeout);
          pending.delete(id);
          reject(e);
        }
      });
    };
  }

  // --- NAP-UPLOAD: upload, with a signing-length wait ----------------------
  //
  // A drop-in for the vendored `upload.upload`: the same envelope (the
  // request, bytes and all, by structured clone — the shell page turns the
  // bytes into something Rust can read), resolved with `msg.result` or
  // rejected with `msg.error` as the vendored one does, but waiting
  // `SIGNING_TIMEOUT_MS` rather than 30 s. The runtime answers once the upload
  // is finished, and the upload's BUD-02 authorization is signed as the user
  // first. `info`, `status` and `onStatus` stay the vendored ones.
  function installUploadUpload(napplet, routers) {
    if (Object.isFrozen(napplet.upload)) {
      console.warn("myco: upload is frozen; uploads keep the 30 s wait");
      return;
    }
    var request = requester(routers, ["upload.upload.result"]);
    napplet.upload.upload = function upload(req) {
      return request({ type: "upload.upload", request: req }, function (msg, resolve, reject) {
        if (msg.result !== undefined) resolve(msg.result);
        else reject(new Error(msg.error || "upload failed"));
      }, SIGNING_TIMEOUT_MS);
    };
  }

  // --- NAP-FS: the pickers, with a person-length wait ---------------------
  //
  // Drop-ins for the vendored `fs.pickFile`, `fs.pickFiles` and
  // `fs.pickSaveFile`: the same envelope, resolved with `msg.result` or
  // rejected with `msg.error` (`cancelled` when the user backs out), as the
  // vendored ones do — but waiting `SIGNING_TIMEOUT_MS` rather than 30 s. The
  // runtime answers once the user has picked, and a person browsing folders
  // routinely takes longer than 30 s. Reads, writes and the rest stay the
  // vendored ones.
  function installFsPick(napplet, routers) {
    if (Object.isFrozen(napplet.fs)) {
      console.warn("myco: fs is frozen; pickers keep the 30 s wait");
      return;
    }
    var request = requester(routers, [
      "fs.pickFile.result",
      "fs.pickFiles.result",
      "fs.pickSaveFile.result"
    ]);
    function picker(type) {
      return function (options) {
        var message = { type: type };
        if (options !== undefined) message.options = options;
        return request(message, function (msg, resolve, reject) {
          if (typeof msg.error === "string") reject(new Error(msg.error));
          else if (msg.result !== undefined) resolve(msg.result);
          else reject(new Error(type + " returned no result"));
        }, SIGNING_TIMEOUT_MS);
      };
    }
    napplet.fs.pickFile = picker("fs.pickFile");
    napplet.fs.pickFiles = picker("fs.pickFiles");
    napplet.fs.pickSaveFile = picker("fs.pickSaveFile");
  }

  // --- NAP-MESH --------------------------------------------------------------
  function installMesh(napplet, routers) {
    var pending = new Map();
    var subscriptions = new Map();

    function post(message) {
      window.parent.postMessage(message, "*");
    }

    function request(message, project, timeoutMs) {
      var id = crypto.randomUUID();
      message.id = id;
      return new Promise(function (resolve, reject) {
        var timeout = setTimeout(function () {
          if (pending.delete(id)) reject(new Error(message.type + " timed out"));
        }, timeoutMs || REQUEST_TIMEOUT_MS);
        pending.set(id, { resolve: resolve, reject: reject, timeout: timeout, project: project });
        post(message);
      });
    }

    routers.push(function (msg) {
      if (msg.type.indexOf("mesh.") !== 0) return;
      if (msg.type === "mesh.event" || msg.type === "mesh.eose" || msg.type === "mesh.closed") {
        var sub = subscriptions.get(msg.subId);
        if (!sub) return;
        if (msg.type === "mesh.event") sub.emit("event", msg.result);
        else if (msg.type === "mesh.eose") sub.emit("eose", msg.ttl);
        else { sub.emit("closed", msg.reason); subscriptions.delete(msg.subId); }
        return;
      }
      if (typeof msg.id !== "string") return;
      var p = pending.get(msg.id);
      if (!p) return;
      pending.delete(msg.id);
      clearTimeout(p.timeout);
      if (typeof msg.error === "string" && msg.type !== "mesh.publish.result") {
        p.reject(new Error(msg.error));
        return;
      }
      p.resolve(p.project(msg));
    });

    function info() {
      return request({ type: "mesh.info" }, function (msg) {
        return { online: !!msg.online, peers: msg.peers | 0, limits: msg.limits || {} };
      });
    }

    // A publish failure is a result with `ok: false`, not a rejection — the
    // napplet reads one field to branch on, as NAP-RELAY has it. Signed
    // first, so it waits as long as an outbox publish does.
    function publish(template, options) {
      var message = { type: "mesh.publish", event: template };
      if (options && options.ttl !== undefined) message.ttl = options.ttl;
      return request(message, function (msg) {
        var out = { ok: !!msg.ok };
        if (msg.event !== undefined) out.event = msg.event;
        if (msg.eventId !== undefined) out.eventId = msg.eventId;
        if (msg.ttl !== undefined) out.ttl = msg.ttl;
        if (msg.error !== undefined) out.error = msg.error;
        return out;
      }, SIGNING_TIMEOUT_MS);
    }

    // How many reachable peers hold each blob: `{ peers, blobs: { <sha256>: n } }`.
    // Rejects with the shell's reason (too many hashes, not granted).
    function blobs(hashes) {
      return request({ type: "mesh.blobs", hashes: Array.isArray(hashes) ? hashes.slice() : [] }, function (msg) {
        return { peers: msg.peers | 0, blobs: msg.blobs || {} };
      });
    }

    function subscribe(filters, options) {
      var subId = crypto.randomUUID();
      var handlers = { event: new Set(), eose: new Set(), closed: new Set() };
      var handle = {
        id: subId,
        on: function (name, cb) {
          var set = handlers[name];
          if (!set) throw new Error("unknown mesh subscription event: " + name);
          set.add(cb);
          return { close: function () { set.delete(cb); } };
        },
        close: function () {
          if (!subscriptions.delete(subId)) return;
          post({ type: "mesh.close", subId: subId });
        },
        emit: function (name, value) {
          handlers[name].forEach(function (cb) {
            try { cb(value); } catch (e) { console.error("mesh subscription handler threw", e); }
          });
        }
      };
      subscriptions.set(subId, handle);
      var message = {
        type: "mesh.subscribe",
        subId: subId,
        filters: Array.isArray(filters) ? filters : [filters]
      };
      if (options && options.ttl !== undefined) message.ttl = options.ttl;
      // A subscribe has no `.result`; its answer is the stream. The `id` is
      // still sent so a refusal (`mesh.subscribe.result` carrying `error`)
      // has something to correlate with, and surfaces as `closed`.
      var id = crypto.randomUUID();
      message.id = id;
      pending.set(id, {
        resolve: function () {},
        reject: function (err) { handle.emit("closed", err.message); subscriptions.delete(subId); },
        timeout: setTimeout(function () { pending.delete(id); }, REQUEST_TIMEOUT_MS),
        project: function () {}
      });
      post(message);
      return { id: subId, on: handle.on, close: handle.close };
    }

    Object.defineProperty(napplet, "mesh", {
      value: Object.freeze({ info: info, publish: publish, subscribe: subscribe, blobs: blobs }),
      enumerable: true, configurable: false, writable: false
    });
  }

  // --- NAP-INTENT: both generations of the binding ---------------------------
  //
  // Current draft:  invoke(uri, options?) / open(uri, options?), where `uri`
  //                 is `napplet:<archetype>/<intent>[?name=value…][#naddr1…]`;
  //                 options are `payload`, `handler`, `behavior`, `handlerHint`.
  // First draft:    invoke({ archetype, action?, convention?, payload?, … }) /
  //                 open(archetype, payload?, options?).
  //
  // An archetype is a slug and never contains `:`, so a string starting
  // `napplet:` is a URI and anything else the first draft. Both send the
  // same `intent.invoke`; the runtime answers with a result carrying both
  // drafts' fields (`handled` and `windowId` beside `handler`). Every
  // invalid URI is a rejected promise, not a throw: a first-draft caller
  // never expected one.
  //
  // `intent.deliver` is held until an `onDelivery` handler exists, then
  // handed to it; a first-draft handler takes the same payload as an
  // `inc.event` on the convention's topic, which the runtime sends as well.
  //
  // Waits `SIGNING_TIMEOUT_MS`: a request is answered once the handler's
  // window is open, which may wait on the user confirming it.
  var BECH32 = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

  function bech32Words(text) {
    var lower = text.toLowerCase();
    if (lower !== text && text.toUpperCase() !== text) throw new Error("mixed-case bech32");
    var sep = lower.lastIndexOf("1");
    if (sep < 1 || sep + 7 > lower.length) throw new Error("malformed bech32");
    var hrp = lower.slice(0, sep);
    var words = [];
    for (var i = sep + 1; i < lower.length; i++) {
      var v = BECH32.indexOf(lower.charAt(i));
      if (v === -1) throw new Error("malformed bech32");
      words.push(v);
    }
    var GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
    var chk = 1;
    function step(v) {
      var top = chk >>> 25;
      chk = ((chk & 0x1ffffff) << 5) ^ v;
      for (var g = 0; g < 5; g++) if ((top >>> g) & 1) chk ^= GEN[g];
    }
    for (var h = 0; h < hrp.length; h++) step(hrp.charCodeAt(h) >>> 5);
    step(0);
    for (h = 0; h < hrp.length; h++) step(hrp.charCodeAt(h) & 31);
    for (h = 0; h < words.length; h++) step(words[h]);
    if (chk !== 1) throw new Error("bad bech32 checksum");
    return { hrp: hrp, words: words.slice(0, -6) };
  }

  function wordsToBytes(words) {
    var acc = 0, bits = 0, out = [];
    for (var i = 0; i < words.length; i++) {
      acc = (acc << 5) | words[i];
      bits += 5;
      while (bits >= 8) {
        bits -= 8;
        out.push((acc >>> bits) & 0xff);
      }
    }
    if (bits >= 5 || ((acc << (8 - bits)) & 0xff)) throw new Error("bad bech32 padding");
    return out;
  }

  function hex(bytes) {
    return bytes.map(function (b) { return (b < 16 ? "0" : "") + b.toString(16); }).join("");
  }

  // A hint's address must be a named napplet: kind 35129, a lowercase-hex
  // pubkey, a non-empty `d` kept exactly as written.
  function validHint(hint) {
    if (typeof hint !== "object" || hint === null ||
        typeof hint.address !== "string" || !/^35129:[0-9a-f]{64}:[\s\S]+$/.test(hint.address)) {
      throw new Error("Intent handler hint requires a named napplet coordinate");
    }
    if (hint.relays !== undefined &&
        !(Array.isArray(hint.relays) && hint.relays.every(function (r) { return typeof r === "string"; }))) {
      throw new Error("Intent handler relay hints must be strings");
    }
    var out = { address: hint.address };
    if (hint.relays && hint.relays.length) out.relays = hint.relays.slice();
    return out;
  }

  // A bare `naddr1…` → `{ kind, address: "<kind>:<hex>:<d>", relays }`.
  function decodeNaddr(text) {
    if (typeof text !== "string" || text.indexOf("naddr1") !== 0) throw new Error("expected a bare naddr");
    var decoded = bech32Words(text);
    if (decoded.hrp !== "naddr") throw new Error("expected a bare naddr");
    var bytes = wordsToBytes(decoded.words);
    var d = null, author = null, kind = null, relays = [];
    for (var i = 0; i + 2 <= bytes.length;) {
      var t = bytes[i], l = bytes[i + 1], v = bytes.slice(i + 2, i + 2 + l);
      if (v.length < l) throw new Error("malformed naddr");
      i += 2 + l;
      var text = new TextDecoder().decode(new Uint8Array(v));
      if (t === 0 && d === null) d = text;
      else if (t === 1) relays.push(text);
      else if (t === 2 && author === null && l === 32) author = hex(v);
      else if (t === 3 && kind === null && l === 4) kind = ((v[0] << 24) | (v[1] << 16) | (v[2] << 8) | v[3]) >>> 0;
    }
    if (d === null || author === null || kind === null) throw new Error("malformed naddr");
    return { kind: kind, d: d, address: kind + ":" + author + ":" + d, relays: relays };
  }

  // `#naddr1…` → `{ address: "35129:<hex>:<d>", relays }`.
  function hintFromNaddr(fragment) {
    var n;
    try { n = decodeNaddr(fragment); } catch (e) {
      throw new Error("Intent handler fragment must be a bare naddr");
    }
    return validHint({ address: n.address, relays: n.relays });
  }

  // `launch`'s target: a named napplet (35129, non-empty `d`) or a root one
  // (15129, empty `d`).
  function launchTarget(naddr) {
    var n = decodeNaddr(naddr);
    if (!((n.kind === 35129 && n.d !== "") || (n.kind === 15129 && n.d === ""))) {
      throw new Error("intent.launch requires a napplet coordinate");
    }
    var target = { address: n.address };
    if (n.relays.length) target.relays = n.relays;
    return target;
  }

  // A convention URI → the normalized wire request.
  function normalizeUri(uri, options) {
    options = options || {};
    var hash = uri.indexOf("#");
    var fragment = hash >= 0 ? uri.slice(hash + 1) : null;
    var rest = hash >= 0 ? uri.slice(0, hash) : uri;
    if (fragment !== null && options.handlerHint !== undefined) {
      throw new Error("Intent handler fragment cannot accompany an explicit handler hint");
    }
    var m = /^napplet:([^/?#]+)\/([^/?#]+)(?:\?(.*))?$/.exec(rest);
    if (!m) throw new Error("invalid convention URI");
    var request = { archetype: m[1], action: m[2], convention: "napplet:" + m[1] + "/" + m[2] };
    if (m[3] !== undefined && m[3] !== "") {
      if (options.payload !== undefined) {
        throw new Error("Intent query parameters cannot accompany an explicit payload");
      }
      var payload = {};
      var pairs = m[3].split("&");
      for (var i = 0; i < pairs.length; i++) {
        var eq = pairs[i].indexOf("=");
        var name = decodeURIComponent(eq >= 0 ? pairs[i].slice(0, eq) : pairs[i]);
        var value = eq >= 0 ? decodeURIComponent(pairs[i].slice(eq + 1)) : "";
        if (Object.prototype.hasOwnProperty.call(payload, name)) {
          throw new Error("Intent query parameter repeated: " + name);
        }
        payload[name] = value;
      }
      request.payload = payload;
    } else if (options.payload !== undefined) {
      request.payload = options.payload;
    }
    if (options.handler !== undefined) request.handler = options.handler;
    if (options.behavior !== undefined) request.behavior = options.behavior;
    if (fragment !== null) request.handlerHint = hintFromNaddr(fragment);
    else if (options.handlerHint !== undefined) request.handlerHint = validHint(options.handlerHint);
    return request;
  }

  // A first-draft request object → the wire request, as the first draft's
  // binding sent it.
  function legacyRequest(req) {
    if (typeof req !== "object" || req === null) throw new Error("invalid intent request");
    var out = {};
    for (var k in req) if (Object.prototype.hasOwnProperty.call(req, k)) out[k] = req[k];
    if (out.handlerHint !== undefined) out.handlerHint = validHint(out.handlerHint);
    return out;
  }

  function isUri(value) {
    return typeof value === "string" && value.indexOf("napplet:") === 0;
  }

  function installIntent(napplet, routers) {
    var request = requester(routers, [
      "intent.invoke.result", "intent.available.result", "intent.handlers.result",
      "intent.launch.result"
    ]);
    var changedHandlers = new Set();
    var deliveryHandlers = new Set();
    var deliveries = [];

    routers.push(function (msg) {
      if (msg.type === "intent.changed") {
        if (!msg.availability) return;
        changedHandlers.forEach(function (h) { h(msg.availability); });
      } else if (msg.type === "intent.deliver") {
        var d = msg.delivery;
        if (typeof d !== "object" || d === null || typeof d.sender !== "string" || !d.sender) return;
        var m = typeof d.convention === "string" ? /^napplet:([^/?#]+)\/([^/?#]+)$/.exec(d.convention) : null;
        if (!m || m[1] !== d.archetype || m[2] !== d.action) return;
        if (deliveryHandlers.size === 0) deliveries.push(d);
        else deliveryHandlers.forEach(function (h) { h(d); });
      }
    });

    function send(wire) {
      return request({ type: "intent.invoke", request: wire }, function (msg, resolve, reject) {
        var r = msg.result;
        if (typeof r === "object" && r !== null && typeof r.ok === "boolean") resolve(r);
        else reject(new Error(msg.error || "invalid intent.invoke.result"));
      }, SIGNING_TIMEOUT_MS);
    }

    function attempt(build) {
      var wire;
      try { wire = build(); } catch (e) { return Promise.reject(e); }
      return send(wire);
    }

    var intent = {
      invoke: function (target, options) {
        return attempt(function () {
          return isUri(target) ? normalizeUri(target, options) : legacyRequest(target);
        });
      },
      open: function (target, payloadOrOptions, legacyOptions) {
        return attempt(function () {
          if (isUri(target)) {
            var wire = normalizeUri(target, payloadOrOptions);
            if (wire.action !== "open") throw new Error("intent.open requires the open URI action");
            return wire;
          }
          var opts = legacyOptions || {};
          var out = { archetype: target, action: "open" };
          if (payloadOrOptions !== undefined) out.payload = payloadOrOptions;
          for (var k in opts) if (Object.prototype.hasOwnProperty.call(opts, k)) out[k] = opts[k];
          return legacyRequest(out);
        });
      },
      // Open the one napplet `naddr` names, or have the shell offer it for
      // install. No payload: data goes through `invoke`.
      launch: function (naddr, options) {
        var message;
        try {
          message = { type: "intent.launch", target: launchTarget(naddr) };
          if (options && options.behavior !== undefined) message.behavior = options.behavior;
        } catch (e) {
          return Promise.reject(e);
        }
        return request(message, function (msg, resolve, reject) {
          var r = msg.result;
          if (typeof r === "object" && r !== null && typeof r.ok === "boolean") resolve(r);
          else reject(new Error(msg.error || "invalid intent.launch.result"));
        }, SIGNING_TIMEOUT_MS);
      },
      available: function (archetype) {
        return request({ type: "intent.available", archetype: archetype }, function (msg, resolve, reject) {
          if (msg.availability !== undefined) resolve(msg.availability);
          else reject(new Error(msg.error || "intent availability unavailable"));
        });
      },
      handlers: function () {
        return request({ type: "intent.handlers" }, function (msg, resolve, reject) {
          if (msg.handlers !== undefined) resolve(msg.handlers);
          else reject(new Error(msg.error || "intent handlers unavailable"));
        });
      },
      onChanged: function (handler) {
        changedHandlers.add(handler);
        return { close: function () { changedHandlers.delete(handler); } };
      },
      onDelivery: function (handler) {
        deliveryHandlers.add(handler);
        var held = deliveries.splice(0);
        for (var i = 0; i < held.length; i++) handler(held[i]);
        return { close: function () { deliveryHandlers.delete(handler); } };
      }
    };
    napplet.intent = intent;
  }

  // --- NAP-LOCAL -------------------------------------------------------------
  //
  // One request/response pair per call, correlated by `id`; `routeTypes` are
  // the result (and error) types this installer answers.
  function requester(routers, routeTypes) {
    var pending = new Map();
    routers.push(function (msg) {
      if (routeTypes.indexOf(msg.type) === -1 || typeof msg.id !== "string") return;
      var p = pending.get(msg.id);
      if (!p) return;
      pending.delete(msg.id);
      clearTimeout(p.timeout);
      p.settle(msg);
    });
    return function request(message, settle, timeoutMs) {
      var id = crypto.randomUUID();
      message.id = id;
      return new Promise(function (resolve, reject) {
        var timeout = setTimeout(function () {
          if (pending.delete(id)) reject(new Error(message.type + " timed out"));
        }, timeoutMs || REQUEST_TIMEOUT_MS);
        pending.set(id, {
          timeout: timeout,
          settle: function (msg) { settle(msg, resolve, reject); }
        });
        try {
          postCloneable(message);
        } catch (e) {
          clearTimeout(timeout);
          pending.delete(id);
          reject(e);
        }
      });
    };
  }

  // `local.publish(eventOrTemplate)` — keep it on this device only. A template
  // is signed as the user; an event the napplet was shown is kept as it is.
  // Resolves `{ ok, event, eventId, error }` like the other publishes.
  function installLocal(napplet, routers) {
    var request = requester(routers, ["local.publish.result"]);
    function publish(event) {
      return request({ type: "local.publish", event: event }, function (msg, resolve) {
        var out = { ok: !!msg.ok };
        if (msg.event !== undefined) out.event = msg.event;
        if (msg.eventId !== undefined) out.eventId = msg.eventId;
        if (msg.error !== undefined) out.error = msg.error;
        resolve(out);
      }, SIGNING_TIMEOUT_MS);
    }
    Object.defineProperty(napplet, "local", {
      value: Object.freeze({ publish: publish }),
      enumerable: true, configurable: false, writable: false
    });
  }

  // `resource.keep(url)` — keep a blob the napplet was shown in this device's
  // own Blossom. Resolves `true`; rejects with the runtime's reason.
  function installResourceKeep(napplet, routers) {
    if (typeof napplet.resource.keep === "function") return;
    if (Object.isFrozen(napplet.resource)) {
      console.warn("myco: resource is frozen; resource.keep is unavailable");
      return;
    }
    var request = requester(routers, ["resource.keep.result", "resource.keep.error"]);
    napplet.resource.keep = function keep(url) {
      return request({ type: "resource.keep", url: url }, function (msg, resolve, reject) {
        if (msg.type === "resource.keep.error") {
          reject(new Error(msg.message || msg.error || "resource.keep refused"));
        } else {
          resolve(true);
        }
      });
    };
  }

  return { install: install };
})();
