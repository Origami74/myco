// Myco's supplement to the vendored `@napplet/shim` prelude.
//
// Trusted code, shipped in the APK, injected into every napplet's srcdoc
// after the vendored installer has run (see `src/prelude.rs`). It fills in
// what the vendored build does not carry:
//
// - `window.napplet.shell` — NAP-SHELL's napplet-side half. The vendored shim
//   has no `shell` domain, so without this `shell.supports()` — the spec's
//   discovery mechanism — is undefined for every domain.
// - `window.napplet.mesh` — NAP-MESH (`docs/design/napplet/NAP-MESH.md`),
//   Myco's own domain, which the vendored installer filters out because it is
//   not in the upstream registry yet.
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
    if (domains.has("mesh")) installMesh(napplet, routers);
    return napplet;
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

  // --- NAP-MESH --------------------------------------------------------------
  function installMesh(napplet, routers) {
    var pending = new Map();
    var subscriptions = new Map();

    function post(message) {
      window.parent.postMessage(message, "*");
    }

    function request(message, project) {
      var id = crypto.randomUUID();
      message.id = id;
      return new Promise(function (resolve, reject) {
        var timeout = setTimeout(function () {
          if (pending.delete(id)) reject(new Error(message.type + " timed out"));
        }, REQUEST_TIMEOUT_MS);
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
    // napplet reads one field to branch on, as NAP-RELAY has it.
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
      value: Object.freeze({ info: info, publish: publish, subscribe: subscribe }),
      enumerable: true, configurable: false, writable: false
    });
  }

  return { install: install };
})();
