"use strict";
var NappletShimPrelude = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // src/prelude.ts
  var prelude_exports = {};
  __export(prelude_exports, {
    install: () => install,
    installNappletRuntimePrelude: () => installNappletRuntimePrelude,
    renderNappletRuntimePreludeCall: () => renderNappletRuntimePreludeCall,
    renderNappletRuntimePreludeScript: () => renderNappletRuntimePreludeScript
  });

  // ../core/dist/index.js
  var NAP_DOMAINS = ["relay", "identity", "storage", "inc", "theme", "keys", "media", "notify", "config", "resource", "cvm", "outbox", "upload", "intent", "ble", "webrtc", "link", "count", "lists", "serial", "fs", "common", "dm"];
  function createDispatch() {
    const handlers2 = /* @__PURE__ */ new Map();
    function registerNap2(domain, handler) {
      if (handlers2.has(domain)) {
        throw new Error(`NAP domain "${domain}" is already registered`);
      }
      handlers2.set(domain, handler);
    }
    function dispatch2(message) {
      const dotIndex = message.type.indexOf(".");
      if (dotIndex <= 0) return false;
      const domain = message.type.slice(0, dotIndex);
      const handler = handlers2.get(domain);
      if (!handler) return false;
      handler(message);
      return true;
    }
    function getRegisteredDomains2() {
      return Array.from(handlers2.keys());
    }
    return { registerNap: registerNap2, dispatch: dispatch2, getRegisteredDomains: getRegisteredDomains2 };
  }
  var _default = createDispatch();
  var registerNap = _default.registerNap;
  var dispatch = _default.dispatch;
  var getRegisteredDomains = _default.getRegisteredDomains;
  var cloneMode = "auto";
  var warnedTypes = /* @__PURE__ */ new Set();
  function isCloneableLeaf(value) {
    if (value instanceof Date || value instanceof RegExp || value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
      return true;
    }
    const tag = Object.prototype.toString.call(value);
    return tag === "[object Blob]" || tag === "[object File]";
  }
  function toCloneableSnapshot(value) {
    return snapshot(value, /* @__PURE__ */ new WeakMap());
  }
  function snapshot(value, seen) {
    if (value === null) return null;
    const type = typeof value;
    if (type === "function" || type === "symbol") {
      throw new TypeError(
        `toCloneableSnapshot: a ${type} is not structured-cloneable and cannot be sent across the napplet boundary`
      );
    }
    if (type !== "object") return value;
    const obj = value;
    const existing = seen.get(obj);
    if (existing !== void 0) return existing;
    if (isCloneableLeaf(obj)) return obj;
    if (Array.isArray(obj)) {
      const out2 = [];
      seen.set(obj, out2);
      for (const item of obj) out2.push(snapshot(item, seen));
      return out2;
    }
    if (obj instanceof Map) {
      const out2 = /* @__PURE__ */ new Map();
      seen.set(obj, out2);
      for (const [k, v] of obj) out2.set(snapshot(k, seen), snapshot(v, seen));
      return out2;
    }
    if (obj instanceof Set) {
      const out2 = /* @__PURE__ */ new Set();
      seen.set(obj, out2);
      for (const v of obj) out2.add(snapshot(v, seen));
      return out2;
    }
    const out = {};
    seen.set(obj, out);
    for (const key of Object.keys(obj)) {
      out[key] = snapshot(obj[key], seen);
    }
    return out;
  }
  function isDataCloneError(err) {
    return err instanceof Error && err.name === "DataCloneError";
  }
  function cloneError(type, cause) {
    const err = new Error(
      `napplet boundary: message "${type}" could not be sent -- an argument is not structured-cloneable (a reactive Proxy?). Svelte 5 $state / Vue reactive / Solid store values can't be postMessage'd. Pass a plain snapshot (e.g. $state.snapshot(x), toCloneableSnapshot(x) from @napplet/core), or call setCloneMode('snapshot') to normalize automatically.`
    );
    err.name = "NappletDataCloneError";
    err.cause = cause;
    return err;
  }
  function sendEnvelope(target, message, targetOrigin = "*") {
    if (cloneMode === "snapshot") {
      let snap;
      try {
        snap = toCloneableSnapshot(message);
      } catch (cause) {
        throw cloneError(message.type, cause);
      }
      try {
        target.postMessage(snap, targetOrigin);
      } catch (cause) {
        if (isDataCloneError(cause)) throw cloneError(message.type, cause);
        throw cause;
      }
      return;
    }
    try {
      target.postMessage(message, targetOrigin);
      return;
    } catch (cause) {
      if (!isDataCloneError(cause)) throw cause;
      if (cloneMode === "strict") throw cloneError(message.type, cause);
      try {
        target.postMessage(toCloneableSnapshot(message), targetOrigin);
      } catch {
        throw cloneError(message.type, cause);
      }
      if (!warnedTypes.has(message.type)) {
        warnedTypes.add(message.type);
        console.warn(
          `napplet boundary: "${message.type}" received a non-cloneable argument (a reactive Proxy?) and was auto-snapshotted before sending. Pass a plain snapshot (e.g. $state.snapshot(x), toCloneableSnapshot(x)) to silence this, or call setCloneMode('strict') to make it throw.`
        );
      }
    }
  }

  // ../nap/dist/chunk-HU72RVEV.js
  function postToShell(message) {
    sendEnvelope(window.parent, message);
  }

  // ../nap/dist/chunk-PC67GBAB.js
  var suppressMap = /* @__PURE__ */ new Map();
  var actionHandlers = /* @__PURE__ */ new Map();
  var pendingRegistrations = /* @__PURE__ */ new Map();
  var installed = false;
  var activeCleanup = null;
  var RESERVED_KEYS = /* @__PURE__ */ new Set(["Tab", "Shift+Tab", "Escape"]);
  function isMessageType(msg, type) {
    return msg.type === type;
  }
  function isTextInput(target) {
    if (!(target instanceof Element)) return false;
    const tag = target.tagName;
    if (tag === "TEXTAREA" || tag === "SELECT") return true;
    if (tag === "INPUT") {
      const type = target.type?.toLowerCase() ?? "text";
      const textTypes = /* @__PURE__ */ new Set([
        "text",
        "search",
        "email",
        "url",
        "password",
        "number",
        "tel",
        "date",
        "datetime-local",
        "month",
        "time",
        "week"
      ]);
      return textTypes.has(type) || type === "";
    }
    if (target.isContentEditable) return true;
    const ce = target.contentEditable;
    if (ce === "true" || ce === "plaintext-only") return true;
    return false;
  }
  function isModifierOnly(key) {
    return key === "Control" || key === "Alt" || key === "Shift" || key === "Meta";
  }
  function normalizeCombo(event) {
    const parts = [];
    if (event.altKey) parts.push("Alt");
    if (event.ctrlKey) parts.push("Ctrl");
    if (event.metaKey) parts.push("Meta");
    if (event.shiftKey) parts.push("Shift");
    parts.push(event.key);
    return parts.join("+");
  }
  function handleBindings(msg) {
    suppressMap.clear();
    for (const binding of msg.bindings) {
      suppressMap.set(binding.key, binding.actionId);
    }
  }
  function handleRegisterResult(msg) {
    const pending4 = pendingRegistrations.get(msg.id);
    if (!pending4) return;
    pendingRegistrations.delete(msg.id);
    if (msg.error) {
      pending4.reject(new Error(msg.error));
      return;
    }
    pending4.resolve({
      actionId: msg.actionId,
      binding: msg.binding
    });
  }
  function handleAction(msg) {
    const handlers2 = actionHandlers.get(msg.actionId);
    if (!handlers2) return;
    for (const cb of handlers2) {
      cb();
    }
  }
  function handleKeydown(event) {
    if (isTextInput(event.target)) return;
    if (isModifierOnly(event.key)) return;
    if (event.isComposing) return;
    const combo = normalizeCombo(event);
    if (!RESERVED_KEYS.has(combo) && suppressMap.has(combo)) {
      event.preventDefault();
      const actionId = suppressMap.get(combo);
      const handlers2 = actionHandlers.get(actionId);
      if (handlers2) {
        for (const cb of handlers2) {
          cb();
        }
      }
      return;
    }
    const msg = {
      type: "keys.forward",
      key: event.key,
      code: event.code,
      ctrl: event.ctrlKey,
      alt: event.altKey,
      shift: event.shiftKey,
      meta: event.metaKey
    };
    postToShell(msg);
  }
  function handleKeysMessage(msg) {
    if (isMessageType(msg, "keys.bindings")) {
      handleBindings(msg);
    } else if (isMessageType(msg, "keys.registerAction.result")) {
      handleRegisterResult(msg);
    } else if (isMessageType(msg, "keys.action")) {
      handleAction(msg);
    }
  }
  function registerAction(action) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      pendingRegistrations.set(id, { resolve, reject });
      const msg = {
        type: "keys.registerAction",
        id,
        action
      };
      postToShell(msg);
      setTimeout(() => {
        if (pendingRegistrations.delete(id)) {
          reject(new Error("keys.registerAction timed out"));
        }
      }, 3e4);
    });
  }
  function unregisterAction(actionId) {
    const msg = {
      type: "keys.unregisterAction",
      actionId
    };
    postToShell(msg);
  }
  function onAction(actionId, callback) {
    if (!actionHandlers.has(actionId)) {
      actionHandlers.set(actionId, /* @__PURE__ */ new Set());
    }
    actionHandlers.get(actionId).add(callback);
    return {
      close() {
        const handlers2 = actionHandlers.get(actionId);
        if (handlers2) {
          handlers2.delete(callback);
          if (handlers2.size === 0) actionHandlers.delete(actionId);
        }
      }
    };
  }
  function installKeysShim() {
    if (installed && activeCleanup) {
      return activeCleanup;
    }
    document.addEventListener("keydown", handleKeydown, true);
    installed = true;
    activeCleanup = () => {
      document.removeEventListener("keydown", handleKeydown, true);
      suppressMap.clear();
      actionHandlers.clear();
      pendingRegistrations.clear();
      installed = false;
      activeCleanup = null;
    };
    return activeCleanup;
  }

  // ../nap/dist/chunk-2IPDBTNH.js
  var pendingCreates = /* @__PURE__ */ new Map();
  var commandHandlers = /* @__PURE__ */ new Map();
  var controlsHandlers = /* @__PURE__ */ new Map();
  var stateHandlers = /* @__PURE__ */ new Map();
  var capabilitiesHandlers = /* @__PURE__ */ new Map();
  var installed2 = false;
  function isMessageType2(msg, type) {
    return msg.type === type;
  }
  function handleCreateResult(msg) {
    const pending4 = pendingCreates.get(msg.id);
    if (!pending4) return;
    pendingCreates.delete(msg.id);
    clearTimeout(pending4.timeout);
    const result = {};
    if (msg.sessionId !== void 0) result.sessionId = msg.sessionId;
    if (msg.owner !== void 0) result.owner = msg.owner;
    if (msg.error !== void 0) result.error = msg.error;
    pending4.resolve(result);
  }
  function handleCommand(msg) {
    const handlers2 = commandHandlers.get(msg.sessionId);
    if (!handlers2) return;
    for (const cb of handlers2) {
      cb(msg.action, msg.value);
    }
  }
  function handleControls(msg) {
    const handlers2 = controlsHandlers.get(msg.sessionId);
    if (!handlers2) return;
    for (const cb of handlers2) {
      cb(msg.controls);
    }
  }
  function handleState(msg) {
    const handlers2 = stateHandlers.get(msg.sessionId);
    if (!handlers2) return;
    const state = {
      status: msg.status,
      position: msg.position,
      duration: msg.duration,
      volume: msg.volume
    };
    for (const cb of handlers2) {
      cb(state);
    }
  }
  function handleCapabilities(msg) {
    const handlers2 = capabilitiesHandlers.get(msg.sessionId);
    if (!handlers2) return;
    for (const cb of handlers2) {
      cb(msg.actions);
    }
  }
  function handleMediaMessage(msg) {
    if (isMessageType2(msg, "media.session.create.result")) {
      handleCreateResult(msg);
    } else if (isMessageType2(msg, "media.command")) {
      handleCommand(msg);
    } else if (isMessageType2(msg, "media.controls")) {
      handleControls(msg);
    } else if (isMessageType2(msg, "media.state")) {
      handleState(msg);
    } else if (isMessageType2(msg, "media.capabilities")) {
      handleCapabilities(msg);
    }
  }
  function createSession(options) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingCreates.delete(id)) {
          reject(new Error("media.session.create timed out"));
        }
      }, 3e4);
      pendingCreates.set(id, { resolve, reject, timeout });
      const msg = {
        type: "media.session.create",
        id,
        ...options
      };
      postToShell(msg);
    });
  }
  function updateSession(sessionId, metadata) {
    const msg = {
      type: "media.session.update",
      sessionId,
      metadata
    };
    postToShell(msg);
  }
  function destroySession(sessionId) {
    const msg = {
      type: "media.session.destroy",
      sessionId
    };
    postToShell(msg);
    commandHandlers.delete(sessionId);
    controlsHandlers.delete(sessionId);
    stateHandlers.delete(sessionId);
    capabilitiesHandlers.delete(sessionId);
  }
  function reportState(sessionId, state) {
    const msg = {
      type: "media.state",
      sessionId,
      ...state
    };
    postToShell(msg);
  }
  function reportCapabilities(sessionId, actions) {
    const msg = {
      type: "media.capabilities",
      sessionId,
      actions
    };
    postToShell(msg);
  }
  function sendCommand(sessionId, action, value) {
    const msg = {
      type: "media.command",
      sessionId,
      action,
      ...value === void 0 ? {} : { value }
    };
    postToShell(msg);
  }
  function onCommand(sessionId, callback) {
    if (!commandHandlers.has(sessionId)) {
      commandHandlers.set(sessionId, /* @__PURE__ */ new Set());
    }
    commandHandlers.get(sessionId).add(callback);
    return {
      close() {
        const handlers2 = commandHandlers.get(sessionId);
        if (handlers2) {
          handlers2.delete(callback);
          if (handlers2.size === 0) commandHandlers.delete(sessionId);
        }
      }
    };
  }
  function onState(sessionId, callback) {
    if (!stateHandlers.has(sessionId)) {
      stateHandlers.set(sessionId, /* @__PURE__ */ new Set());
    }
    stateHandlers.get(sessionId).add(callback);
    return {
      close() {
        const handlers2 = stateHandlers.get(sessionId);
        if (handlers2) {
          handlers2.delete(callback);
          if (handlers2.size === 0) stateHandlers.delete(sessionId);
        }
      }
    };
  }
  function onCapabilities(sessionId, callback) {
    if (!capabilitiesHandlers.has(sessionId)) {
      capabilitiesHandlers.set(sessionId, /* @__PURE__ */ new Set());
    }
    capabilitiesHandlers.get(sessionId).add(callback);
    return {
      close() {
        const handlers2 = capabilitiesHandlers.get(sessionId);
        if (handlers2) {
          handlers2.delete(callback);
          if (handlers2.size === 0) capabilitiesHandlers.delete(sessionId);
        }
      }
    };
  }
  function onControls(sessionId, callback) {
    if (!controlsHandlers.has(sessionId)) {
      controlsHandlers.set(sessionId, /* @__PURE__ */ new Set());
    }
    controlsHandlers.get(sessionId).add(callback);
    return {
      close() {
        const handlers2 = controlsHandlers.get(sessionId);
        if (handlers2) {
          handlers2.delete(callback);
          if (handlers2.size === 0) controlsHandlers.delete(sessionId);
        }
      }
    };
  }
  function installMediaShim() {
    if (installed2) {
      return () => void 0;
    }
    installed2 = true;
    return () => {
      for (const pending4 of pendingCreates.values()) {
        clearTimeout(pending4.timeout);
      }
      pendingCreates.clear();
      commandHandlers.clear();
      controlsHandlers.clear();
      stateHandlers.clear();
      capabilitiesHandlers.clear();
      installed2 = false;
    };
  }

  // ../nap/dist/chunk-2THSWWB3.js
  var REQUEST_TIMEOUT_MS = 3e4;
  var pendingSends = /* @__PURE__ */ new Map();
  var pendingPermissions = /* @__PURE__ */ new Map();
  var actionHandlers2 = /* @__PURE__ */ new Set();
  var clickedHandlers = /* @__PURE__ */ new Set();
  var dismissedHandlers = /* @__PURE__ */ new Set();
  var controlsHandlers2 = /* @__PURE__ */ new Set();
  var installed3 = false;
  function isMessageType3(msg, type) {
    return msg.type === type;
  }
  function handleSendResult(msg) {
    const pending4 = pendingSends.get(msg.id);
    if (!pending4) return;
    pendingSends.delete(msg.id);
    if (msg.error) {
      pending4.reject(new Error(msg.error));
      return;
    }
    if (msg.notificationId) {
      pending4.resolve({ notificationId: msg.notificationId });
    } else {
      pending4.reject(new Error("notify.send.result missing notificationId"));
    }
  }
  function handlePermissionResult(msg) {
    const pending4 = pendingPermissions.get(msg.id);
    if (!pending4) return;
    pendingPermissions.delete(msg.id);
    pending4.resolve({ granted: msg.granted });
  }
  function handleAction2(msg) {
    for (const cb of actionHandlers2) {
      cb(msg.notificationId, msg.actionId);
    }
  }
  function handleClicked(msg) {
    for (const cb of clickedHandlers) {
      cb(msg.notificationId);
    }
  }
  function handleDismissed(msg) {
    for (const cb of dismissedHandlers) {
      cb(msg.notificationId, msg.reason);
    }
  }
  function handleControls2(msg) {
    for (const cb of controlsHandlers2) {
      cb(msg.controls);
    }
  }
  function handleNotifyMessage(msg) {
    if (isMessageType3(msg, "notify.send.result")) {
      handleSendResult(msg);
    } else if (isMessageType3(msg, "notify.permission.result")) {
      handlePermissionResult(msg);
    } else if (isMessageType3(msg, "notify.action")) {
      handleAction2(msg);
    } else if (isMessageType3(msg, "notify.clicked")) {
      handleClicked(msg);
    } else if (isMessageType3(msg, "notify.dismissed")) {
      handleDismissed(msg);
    } else if (isMessageType3(msg, "notify.controls")) {
      handleControls2(msg);
    }
  }
  function send(notification) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      pendingSends.set(id, { resolve, reject });
      const msg = {
        type: "notify.send",
        id,
        ...notification
      };
      postToShell(msg);
      setTimeout(() => {
        if (pendingSends.delete(id)) {
          reject(new Error("notify.send timed out"));
        }
      }, REQUEST_TIMEOUT_MS);
    });
  }
  function dismiss(notificationId) {
    const msg = {
      type: "notify.dismiss",
      notificationId
    };
    postToShell(msg);
  }
  function badge(count) {
    const msg = {
      type: "notify.badge",
      count
    };
    postToShell(msg);
  }
  function registerChannel(channel2) {
    const msg = {
      type: "notify.channel.register",
      ...channel2
    };
    postToShell(msg);
  }
  function requestPermission(channel2) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      pendingPermissions.set(id, { resolve, reject });
      const msg = {
        type: "notify.permission.request",
        id,
        channel: channel2
      };
      postToShell(msg);
      setTimeout(() => {
        if (pendingPermissions.delete(id)) {
          reject(new Error("notify.permission.request timed out"));
        }
      }, REQUEST_TIMEOUT_MS);
    });
  }
  function onAction2(callback) {
    actionHandlers2.add(callback);
    return {
      close() {
        actionHandlers2.delete(callback);
      }
    };
  }
  function onClicked(callback) {
    clickedHandlers.add(callback);
    return {
      close() {
        clickedHandlers.delete(callback);
      }
    };
  }
  function onDismissed(callback) {
    dismissedHandlers.add(callback);
    return {
      close() {
        dismissedHandlers.delete(callback);
      }
    };
  }
  function onControls2(callback) {
    controlsHandlers2.add(callback);
    return {
      close() {
        controlsHandlers2.delete(callback);
      }
    };
  }
  function installNotifyShim() {
    if (installed3) {
      return () => void 0;
    }
    installed3 = true;
    return () => {
      pendingSends.clear();
      pendingPermissions.clear();
      actionHandlers2.clear();
      clickedHandlers.clear();
      dismissedHandlers.clear();
      controlsHandlers2.clear();
      installed3 = false;
    };
  }

  // ../nap/dist/chunk-Q2OL6E2V.js
  var pendingResponses = /* @__PURE__ */ new Map();
  var REQUEST_TIMEOUT_MS2 = 5e3;
  function handleStateResponse(event) {
    if (event.source !== window.parent) return;
    const msg = event.data;
    if (typeof msg !== "object" || msg === null || typeof msg.type !== "string") return;
    if (!msg.type.startsWith("storage.") || !msg.type.endsWith(".result")) return;
    const id = msg.id;
    if (!id) return;
    const pending4 = pendingResponses.get(id);
    if (!pending4) return;
    pendingResponses.delete(id);
    if (msg.error) {
      pending4.reject(new Error(msg.error));
      return;
    }
    pending4.resolve(msg);
  }
  function sendStorageRequest(message) {
    return new Promise((resolve, reject) => {
      pendingResponses.set(message.id, { resolve, reject });
      postToShell(message);
      setTimeout(() => {
        if (pendingResponses.delete(message.id)) {
          reject(new Error("State request timed out"));
        }
      }, REQUEST_TIMEOUT_MS2);
    });
  }
  async function getItemScoped(key, scope) {
    const msg = {
      type: "storage.get",
      id: crypto.randomUUID(),
      key,
      ...scope === "instance" ? { scope } : {}
    };
    const result = await sendStorageRequest(msg);
    return result.value;
  }
  async function setItemScoped(key, value, scope) {
    const msg = {
      type: "storage.set",
      id: crypto.randomUUID(),
      key,
      value,
      ...scope === "instance" ? { scope } : {}
    };
    await sendStorageRequest(msg);
  }
  async function removeItemScoped(key, scope) {
    const msg = {
      type: "storage.remove",
      id: crypto.randomUUID(),
      key,
      ...scope === "instance" ? { scope } : {}
    };
    await sendStorageRequest(msg);
  }
  async function keysScoped(scope) {
    const msg = {
      type: "storage.keys",
      id: crypto.randomUUID(),
      ...scope === "instance" ? { scope } : {}
    };
    const result = await sendStorageRequest(msg);
    return result.keys;
  }
  var nappletStorage = {
    /**
     * Retrieve a stored value by key.
     * Returns null if the key does not exist (matching localStorage semantics).
     *
     * @param key  The state key
     * @returns The stored value, or null if not found
     */
    getItem(key) {
      return getItemScoped(key);
    },
    /**
     * Store a key-value pair.
     *
     * @param key    The state key
     * @param value  The string value to store
     * @throws If the napplet exceeds its 512 KB state quota
     */
    setItem(key, value) {
      return setItemScoped(key, value);
    },
    /**
     * Remove a stored key.
     *
     * @param key  The state key to remove
     */
    removeItem(key) {
      return removeItemScoped(key);
    },
    /**
     * List all keys stored by this napplet.
     *
     * @returns Array of state key strings
     */
    keys() {
      return keysScoped();
    },
    /**
     * Per-instance storage: identical surface to the shared methods, but every
     * request carries `scope: "instance"` so the shell scopes the value to this
     * napplet instance rather than sharing it across instances (NAP-STORAGE).
     */
    instance: {
      /**
       * Retrieve a per-instance value by key. Returns null if not found.
       * @param key  The state key
       */
      getItem(key) {
        return getItemScoped(key, "instance");
      },
      /**
       * Store a per-instance key-value pair.
       * @param key    The state key
       * @param value  The string value to store
       */
      setItem(key, value) {
        return setItemScoped(key, value, "instance");
      },
      /**
       * Remove a per-instance key.
       * @param key  The state key to remove
       */
      removeItem(key) {
        return removeItemScoped(key, "instance");
      },
      /**
       * List all per-instance keys for this napplet instance.
       */
      keys() {
        return keysScoped("instance");
      }
    }
  };
  function installStorageShim() {
    window.addEventListener("message", handleStateResponse);
    return () => {
      window.removeEventListener("message", handleStateResponse);
      pendingResponses.clear();
    };
  }

  // ../nap/dist/chunk-DMVIO3SO.js
  var REQUEST_TIMEOUT_MS3 = 3e4;
  var pendingRequests = /* @__PURE__ */ new Map();
  var changeHandlers = /* @__PURE__ */ new Set();
  var installed4 = false;
  var IDENTITY_MESSAGE_TYPES = /* @__PURE__ */ new Set([
    "identity.getPublicKey.result",
    "identity.getRelays.result",
    "identity.getProfile.result",
    "identity.getFollows.result",
    "identity.getList.result",
    "identity.getZaps.result",
    "identity.getMutes.result",
    "identity.getBlocked.result",
    "identity.getBadges.result",
    "identity.changed",
    "identity.getPublicKey",
    "identity.getRelays",
    "identity.getProfile",
    "identity.getFollows",
    "identity.getList",
    "identity.getZaps",
    "identity.getMutes",
    "identity.getBlocked",
    "identity.getBadges"
  ]);
  function isIdentityNapMessage(msg) {
    return IDENTITY_MESSAGE_TYPES.has(msg.type);
  }
  function handleIdentityMessage(msg) {
    if (!isIdentityNapMessage(msg)) return;
    switch (msg.type) {
      case "identity.getPublicKey.result":
        resolvePending(msg.id, msg.pubkey);
        return;
      case "identity.changed":
        notifyChanged(msg.pubkey);
        return;
      case "identity.getRelays.result":
        resolveOrReject(msg.id, msg.relays, msg.error);
        return;
      case "identity.getProfile.result":
        resolveOrReject(msg.id, msg.profile, msg.error);
        return;
      case "identity.getFollows.result":
        resolveOrReject(msg.id, msg.pubkeys, msg.error);
        return;
      case "identity.getList.result":
        resolveOrReject(msg.id, msg.entries, msg.error);
        return;
      case "identity.getZaps.result":
        resolveOrReject(msg.id, msg.zaps, msg.error);
        return;
      case "identity.getMutes.result":
        resolveOrReject(msg.id, msg.pubkeys, msg.error);
        return;
      case "identity.getBlocked.result":
        resolveOrReject(msg.id, msg.pubkeys, msg.error);
        return;
      case "identity.getBadges.result":
        resolveOrReject(msg.id, msg.badges, msg.error);
        return;
      // ─── Napplet → Shell request messages (defensive — never received here) ──
      case "identity.getPublicKey":
      case "identity.getRelays":
      case "identity.getProfile":
      case "identity.getFollows":
      case "identity.getList":
      case "identity.getZaps":
      case "identity.getMutes":
      case "identity.getBlocked":
      case "identity.getBadges":
        return;
      default:
        assertNever(msg);
        return;
    }
  }
  function resolvePending(id, value) {
    const pending4 = pendingRequests.get(id);
    if (!pending4) return;
    pendingRequests.delete(id);
    clearTimeout(pending4.timeout);
    pending4.resolve(value);
  }
  function resolveOrReject(id, value, error) {
    const pending4 = pendingRequests.get(id);
    if (!pending4) return;
    pendingRequests.delete(id);
    clearTimeout(pending4.timeout);
    if (error) {
      pending4.reject(new Error(error));
    } else {
      pending4.resolve(value);
    }
  }
  function notifyChanged(pubkey) {
    if (typeof pubkey !== "string") return;
    for (const handler of changeHandlers) {
      handler(pubkey);
    }
  }
  function sendRequest(msg) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingRequests.delete(msg.id)) {
          reject(new Error(`${msg.type} timed out`));
        }
      }, REQUEST_TIMEOUT_MS3);
      pendingRequests.set(msg.id, {
        resolve,
        reject,
        timeout
      });
      postToShell(msg);
    });
  }
  function assertNever(_msg) {
  }
  function getPublicKey() {
    const msg = {
      type: "identity.getPublicKey",
      id: crypto.randomUUID()
    };
    return sendRequest(msg);
  }
  function onChanged(handler) {
    changeHandlers.add(handler);
    let closed = false;
    return {
      close() {
        if (closed) return;
        closed = true;
        changeHandlers.delete(handler);
      }
    };
  }
  function getRelays() {
    const msg = {
      type: "identity.getRelays",
      id: crypto.randomUUID()
    };
    return sendRequest(msg);
  }
  function getProfile() {
    const msg = {
      type: "identity.getProfile",
      id: crypto.randomUUID()
    };
    return sendRequest(msg);
  }
  function getFollows() {
    const msg = {
      type: "identity.getFollows",
      id: crypto.randomUUID()
    };
    return sendRequest(msg);
  }
  function getList(listType) {
    const msg = {
      type: "identity.getList",
      id: crypto.randomUUID(),
      listType
    };
    return sendRequest(msg);
  }
  function getZaps() {
    const msg = {
      type: "identity.getZaps",
      id: crypto.randomUUID()
    };
    return sendRequest(msg);
  }
  function getMutes() {
    const msg = {
      type: "identity.getMutes",
      id: crypto.randomUUID()
    };
    return sendRequest(msg);
  }
  function getBlocked() {
    const msg = {
      type: "identity.getBlocked",
      id: crypto.randomUUID()
    };
    return sendRequest(msg);
  }
  function getBadges() {
    const msg = {
      type: "identity.getBadges",
      id: crypto.randomUUID()
    };
    return sendRequest(msg);
  }
  function installIdentityShim() {
    if (installed4) {
      return () => void 0;
    }
    installed4 = true;
    return () => {
      for (const pending4 of pendingRequests.values()) {
        clearTimeout(pending4.timeout);
      }
      pendingRequests.clear();
      changeHandlers.clear();
      installed4 = false;
    };
  }

  // ../nap/dist/chunk-K4GHL555.js
  var REQUEST_TIMEOUT_MS4 = 3e4;
  var pendingRequests2 = /* @__PURE__ */ new Map();
  var changeHandlers2 = /* @__PURE__ */ new Set();
  var installed5 = false;
  var THEME_MESSAGE_TYPES = /* @__PURE__ */ new Set([
    "theme.get.result",
    "theme.changed",
    "theme.get"
  ]);
  function isThemeNapMessage(msg) {
    return THEME_MESSAGE_TYPES.has(msg.type);
  }
  function handleThemeMessage(msg) {
    if (!isThemeNapMessage(msg)) return;
    switch (msg.type) {
      case "theme.get.result":
        resolveOrReject2(msg.id, msg.theme, msg.error);
        return;
      case "theme.changed":
        notifyChanged2(msg.theme);
        return;
      case "theme.get":
        return;
      default:
        assertNever2(msg);
        return;
    }
  }
  function resolveOrReject2(id, value, error) {
    const pending4 = pendingRequests2.get(id);
    if (!pending4) return;
    pendingRequests2.delete(id);
    clearTimeout(pending4.timeout);
    if (error) {
      pending4.reject(new Error(error));
    } else {
      pending4.resolve(value);
    }
  }
  function notifyChanged2(theme) {
    for (const handler of changeHandlers2) {
      handler(theme);
    }
  }
  function sendRequest2(msg) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingRequests2.delete(msg.id)) {
          reject(new Error(`${msg.type} timed out`));
        }
      }, REQUEST_TIMEOUT_MS4);
      pendingRequests2.set(msg.id, {
        resolve,
        reject,
        timeout
      });
      postToShell(msg);
    });
  }
  function assertNever2(_msg) {
  }
  function get() {
    const msg = {
      type: "theme.get",
      id: crypto.randomUUID()
    };
    return sendRequest2(msg);
  }
  function onChanged2(handler) {
    changeHandlers2.add(handler);
    let closed = false;
    return {
      close() {
        if (closed) return;
        closed = true;
        changeHandlers2.delete(handler);
      }
    };
  }
  function installThemeShim() {
    if (installed5) {
      return () => void 0;
    }
    installed5 = true;
    return () => {
      for (const pending4 of pendingRequests2.values()) {
        clearTimeout(pending4.timeout);
      }
      pendingRequests2.clear();
      changeHandlers2.clear();
      installed5 = false;
    };
  }

  // ../nap/dist/chunk-LDVVUZX7.js
  function normalizeConventionUri(uri, explicitPayload) {
    const queryIndex = uri.indexOf("?");
    const fragmentIndex = uri.indexOf("#");
    const pathEnd = [queryIndex, fragmentIndex].filter((index) => index >= 0).reduce((end, index) => Math.min(end, index), uri.length);
    const convention = uri.slice(0, pathEnd);
    const match = /^napplet:([^/?#]+)\/([^/?#]+)$/.exec(convention);
    if (!match) {
      throw new Error("Convention URI must use napplet:<archetype>/<intent> syntax");
    }
    if (fragmentIndex >= 0) {
      throw new Error("Convention URI fragments are not supported");
    }
    const [, archetype, action] = match;
    if (queryIndex < 0) {
      return {
        archetype,
        action,
        convention,
        ...explicitPayload !== void 0 ? { payload: explicitPayload } : {}
      };
    }
    if (explicitPayload !== void 0) {
      throw new Error("Convention URI queries cannot be combined with an explicit payload");
    }
    const names = /* @__PURE__ */ new Set();
    const entries = [];
    const query4 = uri.slice(queryIndex + 1);
    if (query4) {
      for (const pair of query4.split("&")) {
        const separator = pair.indexOf("=");
        if (separator < 0) {
          throw new Error("Convention URI query parameters must use name=value form");
        }
        const name = decodeURIComponent(pair.slice(0, separator));
        const value = decodeURIComponent(pair.slice(separator + 1));
        if (names.has(name)) {
          throw new Error("Convention URI query parameter names must be unique");
        }
        names.add(name);
        entries.push([name, value]);
      }
    }
    return { archetype, action, convention, payload: Object.fromEntries(entries) };
  }
  var REQUEST_TIMEOUT_MS5 = 3e4;
  var MAX_RETAINED_OPENED = 100;
  var MAX_RETAINED_EVENTS = 100;
  var topicHandlers = /* @__PURE__ */ new Map();
  var openedHandlers = /* @__PURE__ */ new Set();
  var retainedOpened = [];
  var channels = /* @__PURE__ */ new Map();
  var pendingOpen = /* @__PURE__ */ new Map();
  var pendingList = /* @__PURE__ */ new Map();
  function transposeConventionUri(topic, payload) {
    const queryIndex = topic.indexOf("?");
    const fragmentIndex = topic.indexOf("#");
    const pathEnd = [queryIndex, fragmentIndex].filter((index) => index >= 0).reduce((end, index) => Math.min(end, index), topic.length);
    const convention = topic.slice(0, pathEnd);
    if (!/^napplet:[^/?#]+\/[^/?#]+$/.test(convention)) {
      return {
        type: "inc.emit",
        topic,
        ...payload !== void 0 ? { payload } : {}
      };
    }
    const normalized = normalizeConventionUri(topic, payload);
    return {
      type: "inc.emit",
      topic: normalized.convention,
      ...normalized.payload !== void 0 ? { payload: normalized.payload } : {}
    };
  }
  function assertStableSubscriptionTopic(topic) {
    const queryIndex = topic.indexOf("?");
    const fragmentIndex = topic.indexOf("#");
    if (queryIndex < 0 && fragmentIndex < 0) return;
    const pathEnd = [queryIndex, fragmentIndex].filter((index) => index >= 0).reduce((end, index) => Math.min(end, index), topic.length);
    if (/^napplet:[^/?#]+\/[^/?#]+$/.test(topic.slice(0, pathEnd))) {
      throw new Error("Convention subscriptions must use the stable queryless topic");
    }
  }
  function closeState(state, reason, notifyRuntime = false) {
    if (state.closed) return;
    if (notifyRuntime) {
      postToShell({ type: "inc.channel.close", channelId: state.info.id });
    }
    state.closed = {
      channelId: state.info.id,
      ...reason !== void 0 ? { reason } : {}
    };
    for (const callback of state.closedHandlers) callback(state.closed);
  }
  function createHandle(channelId, peer) {
    let state = channels.get(channelId);
    if (!state) {
      state = {
        info: { id: channelId, peer },
        events: [],
        eventHandlers: /* @__PURE__ */ new Set(),
        closedHandlers: /* @__PURE__ */ new Set()
      };
      channels.set(channelId, state);
    }
    return {
      id: channelId,
      peer: state.info.peer,
      emit(payload) {
        if (state.closed) return;
        postToShell({
          type: "inc.channel.emit",
          channelId,
          ...payload !== void 0 ? { payload } : {}
        });
      },
      on(callback) {
        state.eventHandlers.add(callback);
        if (state.events.length > 0) {
          const retained = state.events.splice(0);
          for (const event of retained) callback(event);
        }
        return {
          close() {
            state.eventHandlers.delete(callback);
          }
        };
      },
      onClosed(callback) {
        state.closedHandlers.add(callback);
        if (state.closed) callback(state.closed);
        return {
          close() {
            state.closedHandlers.delete(callback);
          }
        };
      },
      close() {
        closeState(state, void 0, true);
      }
    };
  }
  function retainOpened(handle) {
    if (openedHandlers.size > 0) {
      for (const callback of openedHandlers) callback(handle);
      return;
    }
    if (retainedOpened.length >= MAX_RETAINED_OPENED) {
      const overflowed = retainedOpened.shift();
      if (overflowed) {
        const state = channels.get(overflowed.id);
        if (state) closeState(state, "buffer overflow", true);
      }
    }
    retainedOpened.push(handle);
  }
  function emit(topic, payload) {
    postToShell(transposeConventionUri(topic, payload));
  }
  function on(topic, callback) {
    assertStableSubscriptionTopic(topic);
    let handlers2 = topicHandlers.get(topic);
    if (!handlers2) {
      handlers2 = /* @__PURE__ */ new Set();
      topicHandlers.set(topic, handlers2);
    }
    handlers2.add(callback);
    const msg = {
      type: "inc.subscribe",
      id: crypto.randomUUID(),
      topic
    };
    postToShell(msg);
    return {
      close() {
        if (!handlers2.delete(callback)) return;
        if (handlers2.size > 0) return;
        topicHandlers.delete(topic);
        const message = {
          type: "inc.unsubscribe",
          topic
        };
        postToShell(message);
      }
    };
  }
  function open(target) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingOpen.delete(id)) reject(new Error("inc.channel.open timed out"));
      }, REQUEST_TIMEOUT_MS5);
      pendingOpen.set(id, { target, resolve, reject, timeout });
      const message = {
        type: "inc.channel.open",
        id,
        target
      };
      postToShell(message);
    });
  }
  function onOpened(callback) {
    openedHandlers.add(callback);
    if (retainedOpened.length > 0) {
      const retained = retainedOpened.splice(0);
      for (const handle of retained) callback(handle);
    }
    return {
      close() {
        openedHandlers.delete(callback);
      }
    };
  }
  function list() {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingList.delete(id)) reject(new Error("inc.channel.list timed out"));
      }, REQUEST_TIMEOUT_MS5);
      pendingList.set(id, { resolve, reject, timeout });
      const message = {
        type: "inc.channel.list",
        id
      };
      postToShell(message);
    });
  }
  function broadcast(payload) {
    postToShell({
      type: "inc.channel.broadcast",
      ...payload !== void 0 ? { payload } : {}
    });
  }
  var channel = {
    open,
    onOpened,
    list,
    broadcast
  };
  function handleIncEvent(msg) {
    const handlers2 = topicHandlers.get(msg.topic);
    if (!handlers2) return;
    const event = {
      topic: msg.topic,
      sender: msg.sender,
      ..."payload" in msg ? { payload: msg.payload } : {}
    };
    for (const callback of handlers2) callback(event);
  }
  function handleOpenResult(msg) {
    const pending4 = pendingOpen.get(msg.id);
    if (!pending4) return;
    pendingOpen.delete(msg.id);
    clearTimeout(pending4.timeout);
    if (!msg.channelId) {
      pending4.reject(new Error(msg.error ?? "inc.channel.open failed"));
      return;
    }
    pending4.resolve(createHandle(msg.channelId, msg.peer ?? pending4.target));
  }
  function handleOpened(msg) {
    retainOpened(createHandle(msg.channelId, msg.peer));
  }
  function handleChannelEvent(msg) {
    const state = channels.get(msg.channelId);
    if (!state || state.closed) return;
    const event = {
      channelId: msg.channelId,
      sender: msg.sender,
      ..."payload" in msg ? { payload: msg.payload } : {}
    };
    if (state.eventHandlers.size > 0) {
      for (const callback of state.eventHandlers) callback(event);
      return;
    }
    if (state.events.length >= MAX_RETAINED_EVENTS) {
      closeState(state, "buffer overflow", true);
      return;
    }
    state.events.push(event);
  }
  function handleListResult(msg) {
    const pending4 = pendingList.get(msg.id);
    if (!pending4) return;
    pendingList.delete(msg.id);
    clearTimeout(pending4.timeout);
    pending4.resolve(msg.channels);
  }
  function handleClosed(msg) {
    const state = channels.get(msg.channelId);
    if (!state) return;
    closeState(state, msg.reason);
  }
  function handleIncMessage(msg) {
    switch (msg.type) {
      case "inc.event": {
        if (typeof msg.topic !== "string" || typeof msg.sender !== "string") return;
        handleIncEvent({
          type: "inc.event",
          topic: msg.topic,
          sender: msg.sender,
          ..."payload" in msg ? { payload: msg.payload } : {}
        });
        return;
      }
      case "inc.channel.open.result": {
        if (typeof msg.id !== "string") return;
        if (msg.channelId !== void 0 && typeof msg.channelId !== "string") return;
        if (msg.peer !== void 0 && typeof msg.peer !== "string") return;
        if (msg.error !== void 0 && typeof msg.error !== "string") return;
        handleOpenResult({
          type: "inc.channel.open.result",
          id: msg.id,
          ...msg.channelId === void 0 ? {} : { channelId: msg.channelId },
          ...msg.peer === void 0 ? {} : { peer: msg.peer },
          ...msg.error === void 0 ? {} : { error: msg.error }
        });
        return;
      }
      case "inc.channel.opened": {
        if (typeof msg.channelId !== "string" || typeof msg.peer !== "string") return;
        handleOpened({ type: "inc.channel.opened", channelId: msg.channelId, peer: msg.peer });
        return;
      }
      case "inc.channel.event": {
        if (typeof msg.channelId !== "string" || typeof msg.sender !== "string") return;
        handleChannelEvent({
          type: "inc.channel.event",
          channelId: msg.channelId,
          sender: msg.sender,
          ..."payload" in msg ? { payload: msg.payload } : {}
        });
        return;
      }
      case "inc.channel.list.result": {
        if (typeof msg.id !== "string" || !isChannelInfoList(msg.channels)) return;
        handleListResult({ type: "inc.channel.list.result", id: msg.id, channels: msg.channels });
        return;
      }
      case "inc.channel.closed": {
        if (typeof msg.channelId !== "string") return;
        if (msg.reason !== void 0 && typeof msg.reason !== "string") return;
        handleClosed({
          type: "inc.channel.closed",
          channelId: msg.channelId,
          ...msg.reason === void 0 ? {} : { reason: msg.reason }
        });
      }
    }
  }
  function isChannelInfoList(value) {
    return Array.isArray(value) && value.every(
      (entry) => typeof entry === "object" && entry !== null && "id" in entry && typeof entry.id === "string" && "peer" in entry && typeof entry.peer === "string"
    );
  }
  function installIncShim() {
    return () => {
      for (const pending4 of pendingOpen.values()) {
        clearTimeout(pending4.timeout);
        pending4.reject(new Error("INC shim disposed"));
      }
      for (const pending4 of pendingList.values()) {
        clearTimeout(pending4.timeout);
        pending4.reject(new Error("INC shim disposed"));
      }
      for (const state of channels.values()) closeState(state, "endpoint destroyed", true);
      pendingOpen.clear();
      pendingList.clear();
      topicHandlers.clear();
      openedHandlers.clear();
      retainedOpened.length = 0;
      channels.clear();
    };
  }

  // ../nap/dist/chunk-RDIENROU.js
  function isMessageType4(msg, type) {
    return msg.type === type;
  }
  var REQUEST_TIMEOUT_MS6 = 3e4;
  var currentSchema = null;
  var lastValues = null;
  var subscribers = /* @__PURE__ */ new Set();
  var schemaErrorHandlers = /* @__PURE__ */ new Set();
  var pendingGets = /* @__PURE__ */ new Map();
  var pendingRegistrations2 = /* @__PURE__ */ new Map();
  var installed6 = false;
  function handleConfigMessage(msg) {
    if (isMessageType4(msg, "config.registerSchema.result")) {
      handleRegisterSchemaResult(msg);
    } else if (isMessageType4(msg, "config.values")) {
      handleValues(msg);
    } else if (isMessageType4(msg, "config.schemaError")) {
      handleSchemaError(msg);
    }
  }
  function handleRegisterSchemaResult(msg) {
    const pending4 = pendingRegistrations2.get(msg.id);
    if (!pending4) return;
    pendingRegistrations2.delete(msg.id);
    if (msg.ok) {
      currentSchema = pending4.schema;
      pending4.resolve();
    } else {
      const code = msg.code ?? "invalid-schema";
      const error = msg.error ?? "schema rejected";
      pending4.reject(new Error(`${code}: ${error}`));
    }
  }
  function handleValues(msg) {
    lastValues = msg.values;
    if (typeof msg.id === "string") {
      const pending4 = pendingGets.get(msg.id);
      if (!pending4) return;
      pendingGets.delete(msg.id);
      pending4.resolve(msg.values);
      return;
    }
    for (const cb of subscribers) {
      try {
        cb(msg.values);
      } catch {
      }
    }
  }
  function handleSchemaError(msg) {
    const payload = { code: msg.code, error: msg.error };
    for (const cb of schemaErrorHandlers) {
      try {
        cb(payload);
      } catch {
      }
    }
  }
  function registerSchema(schema, version) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      pendingRegistrations2.set(id, { resolve, reject, schema });
      const msg = version === void 0 ? { type: "config.registerSchema", id, schema } : { type: "config.registerSchema", id, schema, version };
      postToShell(msg);
      setTimeout(() => {
        if (pendingRegistrations2.delete(id)) {
          reject(new Error("config.registerSchema timed out"));
        }
      }, REQUEST_TIMEOUT_MS6);
    });
  }
  function get2() {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      pendingGets.set(id, { resolve, reject });
      const msg = { type: "config.get", id };
      postToShell(msg);
      setTimeout(() => {
        if (pendingGets.delete(id)) {
          reject(new Error("config.get timed out"));
        }
      }, REQUEST_TIMEOUT_MS6);
    });
  }
  function subscribe(callback) {
    const firstSubscriber = subscribers.size === 0;
    subscribers.add(callback);
    if (firstSubscriber) {
      const msg = { type: "config.subscribe" };
      postToShell(msg);
    } else if (lastValues !== null) {
      const snapshot2 = lastValues;
      queueMicrotask(() => {
        if (subscribers.has(callback) && snapshot2 !== null) {
          try {
            callback(snapshot2);
          } catch {
          }
        }
      });
    }
    return {
      close() {
        const existed = subscribers.delete(callback);
        if (existed && subscribers.size === 0) {
          const msg = { type: "config.unsubscribe" };
          postToShell(msg);
        }
      }
    };
  }
  function openSettings(options) {
    const section = options?.section;
    const msg = section === void 0 ? { type: "config.openSettings" } : { type: "config.openSettings", section };
    postToShell(msg);
  }
  function onSchemaError(callback) {
    schemaErrorHandlers.add(callback);
    return () => {
      schemaErrorHandlers.delete(callback);
    };
  }
  function installConfigShim() {
    if (installed6) {
      return () => void 0;
    }
    const configWindow = window;
    const napplet = configWindow.napplet ?? (configWindow.napplet = {});
    const api = {
      registerSchema,
      get: get2,
      subscribe,
      openSettings,
      onSchemaError
    };
    Object.defineProperty(api, "schema", {
      get: () => currentSchema,
      enumerable: true,
      configurable: false
    });
    napplet.config = api;
    installed6 = true;
    return () => {
      subscribers.clear();
      schemaErrorHandlers.clear();
      pendingGets.clear();
      pendingRegistrations2.clear();
      lastValues = null;
      currentSchema = null;
      if (napplet.config === api) delete napplet.config;
      installed6 = false;
    };
  }

  // ../nap/dist/chunk-U4MBOGHQ.js
  var REQUEST_TIMEOUT_MS7 = 3e4;
  var inflight = /* @__PURE__ */ new Map();
  var pendingBytes = /* @__PURE__ */ new Map();
  var pendingInfo = /* @__PURE__ */ new Map();
  var pendingMany = /* @__PURE__ */ new Map();
  var installed7 = false;
  function isMessageType5(msg, type) {
    return msg.type === type;
  }
  function handleResourceMessage(msg) {
    if (isMessageType5(msg, "resource.info.result")) {
      const result = msg;
      const p = pendingInfo.get(result.id);
      if (!p) return;
      pendingInfo.delete(result.id);
      clearTimeout(p.timeout);
      p.resolve(result.info);
    } else if (isMessageType5(msg, "resource.info.error")) {
      const err = msg;
      const p = pendingInfo.get(err.id);
      if (!p) return;
      pendingInfo.delete(err.id);
      clearTimeout(p.timeout);
      p.reject(new Error(err.message ? `${err.error}: ${err.message}` : err.error));
    } else if (isMessageType5(msg, "resource.bytes.result")) {
      const result = msg;
      const p = pendingBytes.get(result.id);
      if (!p) return;
      pendingBytes.delete(result.id);
      clearTimeout(p.timeout);
      p.resolve(result.blob);
    } else if (isMessageType5(msg, "resource.bytes.error")) {
      const err = msg;
      const p = pendingBytes.get(err.id);
      if (!p) return;
      pendingBytes.delete(err.id);
      clearTimeout(p.timeout);
      p.reject(new Error(err.message ? `${err.error}: ${err.message}` : err.error));
    } else if (isMessageType5(msg, "resource.bytesMany.result")) {
      const result = msg;
      const p = pendingMany.get(result.id);
      if (!p) return;
      pendingMany.delete(result.id);
      clearTimeout(p.timeout);
      p.resolve(result.items);
    } else if (isMessageType5(msg, "resource.bytesMany.error")) {
      const err = msg;
      const p = pendingMany.get(err.id);
      if (!p) return;
      pendingMany.delete(err.id);
      clearTimeout(p.timeout);
      p.reject(new Error(err.message ? `${err.error}: ${err.message}` : err.error));
    }
  }
  function sendInfoRequest(id) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingInfo.delete(id)) {
          reject(new Error("resource.info timed out"));
        }
      }, REQUEST_TIMEOUT_MS7);
      pendingInfo.set(id, { resolve, reject, timeout });
      const msg = {
        type: "resource.info",
        id
      };
      postToShell(msg);
    });
  }
  function sendBytesRequest(url, id) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingBytes.delete(id)) {
          reject(new Error(`resource.bytes timed out for ${url}`));
        }
      }, REQUEST_TIMEOUT_MS7);
      pendingBytes.set(id, { resolve, reject, timeout });
      const msg = {
        type: "resource.bytes",
        id,
        url
      };
      postToShell(msg);
    });
  }
  function sendBytesManyRequest(urls, id) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingMany.delete(id)) {
          reject(new Error(`resource.bytesMany timed out for ${urls.length} URLs`));
        }
      }, REQUEST_TIMEOUT_MS7);
      pendingMany.set(id, { resolve, reject, timeout });
      const msg = {
        type: "resource.bytesMany",
        id,
        urls
      };
      postToShell(msg);
    });
  }
  function sendCancel(id) {
    const msg = {
      type: "resource.cancel",
      id
    };
    postToShell(msg);
  }
  function isHexByte(value) {
    return /^[0-9a-fA-F]{2}$/.test(value);
  }
  function percentDecodeToBytes(value) {
    const bytes2 = [];
    const encoder = new TextEncoder();
    for (let i = 0; i < value.length; i += 1) {
      const char = value[i];
      if (char === "%" && isHexByte(value.slice(i + 1, i + 3))) {
        bytes2.push(Number.parseInt(value.slice(i + 1, i + 3), 16));
        i += 2;
        continue;
      }
      bytes2.push(...encoder.encode(char));
    }
    return new Uint8Array(bytes2);
  }
  function percentDecodeToString(value) {
    let output = "";
    for (let i = 0; i < value.length; i += 1) {
      const char = value[i];
      if (char === "%" && isHexByte(value.slice(i + 1, i + 3))) {
        output += String.fromCharCode(Number.parseInt(value.slice(i + 1, i + 3), 16));
        i += 2;
        continue;
      }
      output += char;
    }
    return output;
  }
  function base64ToBytes(value) {
    const decoded = atob(percentDecodeToString(value).replace(/\s/g, ""));
    const bytes2 = new Uint8Array(decoded.length);
    for (let i = 0; i < decoded.length; i += 1) {
      bytes2[i] = decoded.charCodeAt(i);
    }
    return bytes2;
  }
  function toArrayBuffer(bytes2) {
    const buffer = new ArrayBuffer(bytes2.byteLength);
    new Uint8Array(buffer).set(bytes2);
    return buffer;
  }
  function decodeDataUrl(url) {
    const comma = url.indexOf(",");
    if (comma < 0) return Promise.reject(new Error("invalid data URL"));
    const metadata = url.slice("data:".length, comma);
    const data = url.slice(comma + 1);
    const parts = metadata.split(";").filter((part) => part.length > 0);
    const isBase64 = parts.some((part) => part.toLowerCase() === "base64");
    const type = parts.filter((part) => part.toLowerCase() !== "base64").join(";");
    try {
      const bytes2 = isBase64 ? base64ToBytes(data) : percentDecodeToBytes(data);
      return Promise.resolve(new Blob([toArrayBuffer(bytes2)], { type }));
    } catch (error) {
      return Promise.reject(error instanceof Error ? error : new Error("invalid data URL"));
    }
  }
  function wireSignal(work, signal, cancelId = null, cancelPending) {
    if (!signal) return work;
    if (signal.aborted) {
      const error = new DOMException("Aborted", "AbortError");
      if (cancelId) sendCancel(cancelId);
      cancelPending?.(error);
      return Promise.reject(error);
    }
    return new Promise((resolve, reject) => {
      const onAbort = () => {
        const error = new DOMException("Aborted", "AbortError");
        if (cancelId) sendCancel(cancelId);
        cancelPending?.(error);
        reject(error);
      };
      signal.addEventListener("abort", onAbort, { once: true });
      work.then(
        (b) => {
          signal.removeEventListener("abort", onAbort);
          resolve(b);
        },
        (e) => {
          signal.removeEventListener("abort", onAbort);
          reject(e);
        }
      );
    });
  }
  function cancelBytes(id, reason) {
    const p = pendingBytes.get(id);
    if (!p) return;
    pendingBytes.delete(id);
    clearTimeout(p.timeout);
    p.reject(reason);
  }
  function cancelMany(id, reason) {
    const p = pendingMany.get(id);
    if (!p) return;
    pendingMany.delete(id);
    clearTimeout(p.timeout);
    p.reject(reason);
  }
  function wireManySignal(work, signal, cancelId = null) {
    if (!signal) return work;
    if (signal.aborted) {
      const error = new DOMException("Aborted", "AbortError");
      if (cancelId) sendCancel(cancelId);
      if (cancelId) cancelMany(cancelId, error);
      return Promise.reject(error);
    }
    return new Promise((resolve, reject) => {
      const onAbort = () => {
        const error = new DOMException("Aborted", "AbortError");
        if (cancelId) sendCancel(cancelId);
        if (cancelId) cancelMany(cancelId, error);
        reject(error);
      };
      signal.addEventListener("abort", onAbort, { once: true });
      work.then(
        (items) => {
          signal.removeEventListener("abort", onAbort);
          resolve(items);
        },
        (e) => {
          signal.removeEventListener("abort", onAbort);
          reject(e);
        }
      );
    });
  }
  function installResourceShim() {
    if (installed7) {
      return () => void 0;
    }
    installed7 = true;
    return () => {
      inflight.clear();
      pendingInfo.clear();
      pendingBytes.clear();
      pendingMany.clear();
      installed7 = false;
    };
  }
  function info() {
    return sendInfoRequest(crypto.randomUUID());
  }
  function bytes(url, opts) {
    if (opts?.signal?.aborted) {
      return Promise.reject(new DOMException("Aborted", "AbortError"));
    }
    const cached = inflight.get(url);
    if (cached) {
      return wireSignal(cached, opts?.signal);
    }
    let work;
    let cancelId = null;
    try {
      const protocol = new URL(url).protocol;
      if (protocol === "data:") {
        work = decodeDataUrl(url);
      } else {
        cancelId = crypto.randomUUID();
        work = sendBytesRequest(url, cancelId);
      }
    } catch {
      return Promise.reject(new Error(`invalid URL: ${url}`));
    }
    work = work.finally(() => {
      inflight.delete(url);
    });
    inflight.set(url, work);
    return wireSignal(
      work,
      opts?.signal,
      cancelId,
      cancelId ? (reason) => cancelBytes(cancelId, reason) : void 0
    );
  }
  function bytesMany(urls, opts) {
    if (urls.length === 0) {
      return Promise.reject(new Error("invalid-request: urls must be non-empty"));
    }
    if (opts?.signal?.aborted) {
      return Promise.reject(new DOMException("Aborted", "AbortError"));
    }
    const id = crypto.randomUUID();
    const work = sendBytesManyRequest(urls, id);
    return wireManySignal(work, opts?.signal, id);
  }
  function bytesAsObjectURL(url) {
    const handle = { url: "", revoke: () => {
    } };
    let objectUrl = null;
    let revoked = false;
    const ready = bytes(url).then((blob) => {
      if (revoked) return;
      objectUrl = URL.createObjectURL(blob);
      handle.url = objectUrl;
      return objectUrl;
    });
    handle.revoke = () => {
      if (revoked) return;
      revoked = true;
      if (objectUrl) {
        URL.revokeObjectURL(objectUrl);
        objectUrl = null;
      }
    };
    Object.defineProperty(handle, "ready", {
      value: ready,
      enumerable: false,
      writable: false
    });
    return handle;
  }
  function hydrateResourceCache(entries) {
    if (!entries || entries.length === 0) return;
    for (const entry of entries) {
      inflight.set(entry.url, Promise.resolve(entry.blob));
    }
  }

  // ../nap/dist/chunk-BFHWB3MF.js
  var REQUEST_TIMEOUT_MS8 = 3e4;
  var pendingDiscover = /* @__PURE__ */ new Map();
  var pendingRequest = /* @__PURE__ */ new Map();
  var pendingClose = /* @__PURE__ */ new Map();
  var pendingRegistryList = /* @__PURE__ */ new Map();
  var pendingRegistryHas = /* @__PURE__ */ new Map();
  var pendingRegistryDescribe = /* @__PURE__ */ new Map();
  var pendingRegistryCall = /* @__PURE__ */ new Map();
  var eventHandlers = /* @__PURE__ */ new Set();
  var installed8 = false;
  function setInstalled(value) {
    installed8 = value;
  }
  function isMessageType6(msg, type) {
    return msg.type === type;
  }
  function handleDiscoverResult(msg) {
    const p = pendingDiscover.get(msg.id);
    if (!p) return;
    pendingDiscover.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.error !== void 0) {
      p.reject(new Error(msg.error));
      return;
    }
    p.resolve(Array.isArray(msg.servers) ? msg.servers : []);
  }
  function handleRequestResult(msg) {
    const p = pendingRequest.get(msg.id);
    if (!p) return;
    pendingRequest.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.error !== void 0) {
      p.reject(new Error(msg.error));
      return;
    }
    if (msg.message === void 0) {
      p.reject(new Error("cvm.request.result missing message"));
      return;
    }
    p.resolve(msg.message);
  }
  function handleCloseResult(msg) {
    const p = pendingClose.get(msg.id);
    if (!p) return;
    pendingClose.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.error !== void 0) {
      p.reject(new Error(msg.error));
      return;
    }
    p.resolve();
  }
  function handleEvent(msg) {
    if (!msg.server || !msg.message) return;
    for (const cb of eventHandlers) {
      cb(msg.server, msg.message);
    }
  }
  function handleRegistryListResult(msg) {
    const p = pendingRegistryList.get(msg.id);
    if (!p) return;
    pendingRegistryList.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.error !== void 0) {
      p.reject(new Error(msg.error));
      return;
    }
    p.resolve(Array.isArray(msg.entries) ? msg.entries : []);
  }
  function handleRegistryHasResult(msg) {
    const p = pendingRegistryHas.get(msg.id);
    if (!p) return;
    pendingRegistryHas.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.error !== void 0) {
      p.reject(new Error(msg.error));
      return;
    }
    p.resolve(msg.has === true);
  }
  function handleRegistryDescribeResult(msg) {
    const p = pendingRegistryDescribe.get(msg.id);
    if (!p) return;
    pendingRegistryDescribe.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.error !== void 0) {
      p.reject(new Error(msg.error));
      return;
    }
    if (msg.entry === void 0) {
      p.reject(new Error("cvm.registry.describe.result missing entry"));
      return;
    }
    p.resolve(msg.entry);
  }
  function handleRegistryCallResult(msg) {
    const p = pendingRegistryCall.get(msg.id);
    if (!p) return;
    pendingRegistryCall.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.error !== void 0) {
      p.reject(new Error(msg.error));
      return;
    }
    if (msg.result === void 0) {
      p.reject(new Error("cvm.registry.call.result missing result"));
      return;
    }
    p.resolve(msg.result);
  }
  function handleCvmMessage(msg) {
    if (isMessageType6(msg, "cvm.discover.result")) {
      handleDiscoverResult(msg);
    } else if (isMessageType6(msg, "cvm.request.result")) {
      handleRequestResult(msg);
    } else if (isMessageType6(msg, "cvm.close.result")) {
      handleCloseResult(msg);
    } else if (isMessageType6(msg, "cvm.event")) {
      handleEvent(msg);
    } else if (isMessageType6(msg, "cvm.registry.list.result")) {
      handleRegistryListResult(msg);
    } else if (isMessageType6(msg, "cvm.registry.has.result")) {
      handleRegistryHasResult(msg);
    } else if (isMessageType6(msg, "cvm.registry.describe.result")) {
      handleRegistryDescribeResult(msg);
    } else if (isMessageType6(msg, "cvm.registry.call.result")) {
      handleRegistryCallResult(msg);
    }
  }
  function discover(query4) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingDiscover.delete(id)) reject(new Error("cvm.discover timed out"));
      }, REQUEST_TIMEOUT_MS8);
      pendingDiscover.set(id, { resolve, reject, timeout });
      const msg = {
        type: "cvm.discover",
        id,
        ...query4 === void 0 ? {} : { query: query4 }
      };
      postToShell(msg);
    });
  }
  function request(server, message, options) {
    const id = crypto.randomUUID();
    const timeoutMs = options?.timeoutMs ?? REQUEST_TIMEOUT_MS8;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingRequest.delete(id)) reject(new Error("cvm.request timed out"));
      }, timeoutMs);
      pendingRequest.set(id, { resolve, reject, timeout });
      const msg = {
        type: "cvm.request",
        id,
        server,
        message,
        ...options === void 0 ? {} : { options }
      };
      postToShell(msg);
    });
  }
  async function mcpCall(server, method, params, options) {
    const reply = await request(
      server,
      {
        jsonrpc: "2.0",
        id: crypto.randomUUID(),
        method,
        ...params === void 0 ? {} : { params }
      },
      options
    );
    if (reply.error !== void 0) {
      const err = reply.error;
      throw new Error(err?.message ? `${method}: ${err.message}` : `${method} failed`);
    }
    return reply.result;
  }
  async function listTools(server, options) {
    const result = await mcpCall(server, "tools/list", void 0, options);
    return result?.tools ?? [];
  }
  function callTool(server, name, args, options) {
    return mcpCall(
      server,
      "tools/call",
      { name, ...args === void 0 ? {} : { arguments: args } },
      options
    );
  }
  async function listResources(server, options) {
    const result = await mcpCall(server, "resources/list", void 0, options);
    return result?.resources ?? [];
  }
  async function readResource(server, uri, options) {
    const result = await mcpCall(
      server,
      "resources/read",
      { uri },
      options
    );
    const first = result?.contents?.[0];
    if (!first) throw new Error(`resources/read returned no contents for ${uri}`);
    return first;
  }
  function close(server) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingClose.delete(id)) reject(new Error("cvm.close timed out"));
      }, REQUEST_TIMEOUT_MS8);
      pendingClose.set(id, { resolve, reject, timeout });
      const msg = {
        type: "cvm.close",
        id,
        server
      };
      postToShell(msg);
    });
  }
  function onEvent(callback) {
    eventHandlers.add(callback);
    return {
      close() {
        eventHandlers.delete(callback);
      }
    };
  }
  function registryList(query4) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingRegistryList.delete(id)) reject(new Error("cvm.registry.list timed out"));
      }, REQUEST_TIMEOUT_MS8);
      pendingRegistryList.set(id, { resolve, reject, timeout });
      const msg = {
        type: "cvm.registry.list",
        id,
        ...query4 === void 0 ? {} : { query: query4 }
      };
      postToShell(msg);
    });
  }
  function registryHas(family, options) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingRegistryHas.delete(id)) reject(new Error("cvm.registry.has timed out"));
      }, REQUEST_TIMEOUT_MS8);
      pendingRegistryHas.set(id, { resolve, reject, timeout });
      const msg = {
        type: "cvm.registry.has",
        id,
        family,
        ...options === void 0 ? {} : { options }
      };
      postToShell(msg);
    });
  }
  function registryDescribe(family, options) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingRegistryDescribe.delete(id)) reject(new Error("cvm.registry.describe timed out"));
      }, REQUEST_TIMEOUT_MS8);
      pendingRegistryDescribe.set(id, { resolve, reject, timeout });
      const msg = {
        type: "cvm.registry.describe",
        id,
        family,
        ...options === void 0 ? {} : { options }
      };
      postToShell(msg);
    });
  }
  function registryCall(family, tool, args, options) {
    const id = crypto.randomUUID();
    const timeoutMs = options?.timeoutMs ?? REQUEST_TIMEOUT_MS8;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingRegistryCall.delete(id)) reject(new Error("cvm.registry.call timed out"));
      }, timeoutMs);
      pendingRegistryCall.set(id, { resolve, reject, timeout });
      const msg = {
        type: "cvm.registry.call",
        id,
        family,
        tool,
        ...args === void 0 ? {} : { args },
        ...options === void 0 ? {} : { options }
      };
      postToShell(msg);
    });
  }
  function installCvmShim() {
    if (installed8) return () => void 0;
    setInstalled(true);
    return () => {
      for (const pending4 of [
        ...pendingDiscover.values(),
        ...pendingRequest.values(),
        ...pendingClose.values(),
        ...pendingRegistryList.values(),
        ...pendingRegistryHas.values(),
        ...pendingRegistryDescribe.values(),
        ...pendingRegistryCall.values()
      ]) clearTimeout(pending4.timeout);
      pendingDiscover.clear();
      pendingRequest.clear();
      pendingClose.clear();
      pendingRegistryList.clear();
      pendingRegistryHas.clear();
      pendingRegistryDescribe.clear();
      pendingRegistryCall.clear();
      eventHandlers.clear();
      setInstalled(false);
    };
  }

  // ../nap/dist/chunk-HNZ6MTKU.js
  var REQUEST_TIMEOUT_MS9 = 3e4;
  var pendingGetEvent = /* @__PURE__ */ new Map();
  var pendingQuery = /* @__PURE__ */ new Map();
  var pendingPublish = /* @__PURE__ */ new Map();
  var pendingResolve = /* @__PURE__ */ new Map();
  var subscriptions = /* @__PURE__ */ new Map();
  var installed9 = false;
  function isMessageType7(msg, type) {
    return msg.type === type;
  }
  function handleGetEventResult(msg) {
    const p = pendingGetEvent.get(msg.id);
    if (!p) return;
    pendingGetEvent.delete(msg.id);
    clearTimeout(p.timeout);
    const result = {};
    if (msg.result !== void 0) {
      hydrateResourceCache(msg.result.sidecar?.resources);
      result.result = msg.result;
    }
    if (msg.incomplete !== void 0) result.incomplete = msg.incomplete;
    if (msg.error !== void 0) result.error = msg.error;
    p.resolve(result);
  }
  function handleQueryResult(msg) {
    const p = pendingQuery.get(msg.id);
    if (!p) return;
    pendingQuery.delete(msg.id);
    clearTimeout(p.timeout);
    const events = Array.isArray(msg.events) ? msg.events : [];
    for (const item of events) hydrateResourceCache(item.sidecar?.resources);
    const result = {
      events
    };
    if (msg.incomplete !== void 0) result.incomplete = msg.incomplete;
    if (msg.error !== void 0) result.error = msg.error;
    p.resolve(result);
  }
  function handlePublishResult(msg) {
    const p = pendingPublish.get(msg.id);
    if (!p) return;
    pendingPublish.delete(msg.id);
    clearTimeout(p.timeout);
    const result = { ok: msg.ok };
    if (msg.event !== void 0) result.event = msg.event;
    if (msg.eventId !== void 0) result.eventId = msg.eventId;
    if (msg.relays !== void 0) result.relays = msg.relays;
    if (msg.error !== void 0) result.error = msg.error;
    p.resolve(result);
  }
  function handleResolveResult(msg) {
    const p = pendingResolve.get(msg.id);
    if (!p) return;
    pendingResolve.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.error !== void 0) {
      p.reject(new Error(msg.error));
      return;
    }
    if (!msg.plan) {
      p.reject(new Error("outbox.resolveRelays.result missing plan"));
      return;
    }
    p.resolve(msg.plan);
  }
  function handleSubEvent(msg) {
    const sub = subscriptions.get(msg.subId);
    if (!sub) return;
    hydrateResourceCache(msg.result.sidecar?.resources);
    for (const cb of sub.event) cb(msg.result);
  }
  function handleSubClosed(msg) {
    const sub = subscriptions.get(msg.subId);
    if (!sub) return;
    for (const cb of sub.closed) cb(msg.reason);
    subscriptions.delete(msg.subId);
  }
  function handleOutboxMessage(msg) {
    if (isMessageType7(msg, "outbox.getEvent.result")) {
      handleGetEventResult(msg);
    } else if (isMessageType7(msg, "outbox.query.result")) {
      handleQueryResult(msg);
    } else if (isMessageType7(msg, "outbox.publish.result")) {
      handlePublishResult(msg);
    } else if (isMessageType7(msg, "outbox.resolveRelays.result")) {
      handleResolveResult(msg);
    } else if (isMessageType7(msg, "outbox.event")) {
      handleSubEvent(msg);
    } else if (isMessageType7(msg, "outbox.closed")) {
      handleSubClosed(msg);
    }
  }
  function getEvent(eventId, options) {
    const id = crypto.randomUUID();
    const timeoutMs = options?.timeoutMs ?? REQUEST_TIMEOUT_MS9;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingGetEvent.delete(id)) reject(new Error("outbox.getEvent timed out"));
      }, timeoutMs);
      pendingGetEvent.set(id, { resolve, reject, timeout });
      const msg = {
        type: "outbox.getEvent",
        id,
        eventId,
        ...options === void 0 ? {} : { options }
      };
      postToShell(msg);
    });
  }
  function query(filters, options) {
    const id = crypto.randomUUID();
    const timeoutMs = options?.timeoutMs ?? REQUEST_TIMEOUT_MS9;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingQuery.delete(id)) reject(new Error("outbox.query timed out"));
      }, timeoutMs);
      pendingQuery.set(id, { resolve, reject, timeout });
      const msg = {
        type: "outbox.query",
        id,
        filters,
        ...options === void 0 ? {} : { options }
      };
      postToShell(msg);
    });
  }
  function subscribe2(filters, options) {
    const id = crypto.randomUUID();
    const subId = crypto.randomUUID();
    const listeners = {
      event: /* @__PURE__ */ new Set(),
      closed: /* @__PURE__ */ new Set()
    };
    subscriptions.set(subId, listeners);
    const msg = {
      type: "outbox.subscribe",
      id,
      subId,
      filters,
      ...options === void 0 ? {} : { options }
    };
    postToShell(msg);
    function on2(event, cb) {
      if (event === "event") listeners.event.add(cb);
      else if (event === "closed") listeners.closed.add(cb);
    }
    return {
      on: on2,
      close() {
        if (!subscriptions.delete(subId)) return;
        const closeMsg = {
          type: "outbox.close",
          id: crypto.randomUUID(),
          subId
        };
        postToShell(closeMsg);
      }
    };
  }
  function publish(template, options) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingPublish.delete(id)) reject(new Error("outbox.publish timed out"));
      }, REQUEST_TIMEOUT_MS9);
      pendingPublish.set(id, { resolve, reject, timeout });
      const msg = {
        type: "outbox.publish",
        id,
        event: template,
        ...options === void 0 ? {} : { options }
      };
      postToShell(msg);
    });
  }
  function resolveRelays(target) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingResolve.delete(id)) reject(new Error("outbox.resolveRelays timed out"));
      }, REQUEST_TIMEOUT_MS9);
      pendingResolve.set(id, { resolve, reject, timeout });
      const msg = {
        type: "outbox.resolveRelays",
        id,
        target
      };
      postToShell(msg);
    });
  }
  function installOutboxShim() {
    if (installed9) {
      return () => void 0;
    }
    installed9 = true;
    return () => {
      for (const p of pendingGetEvent.values()) clearTimeout(p.timeout);
      for (const p of pendingQuery.values()) clearTimeout(p.timeout);
      for (const p of pendingPublish.values()) clearTimeout(p.timeout);
      for (const p of pendingResolve.values()) clearTimeout(p.timeout);
      pendingGetEvent.clear();
      pendingQuery.clear();
      pendingPublish.clear();
      pendingResolve.clear();
      subscriptions.clear();
      installed9 = false;
    };
  }

  // ../nap/dist/chunk-QCYYTDDB.js
  var REQUEST_TIMEOUT_MS10 = 3e4;
  var pendingInfo2 = /* @__PURE__ */ new Map();
  var pendingUpload = /* @__PURE__ */ new Map();
  var pendingStatus = /* @__PURE__ */ new Map();
  var statusHandlers = /* @__PURE__ */ new Set();
  var installed10 = false;
  function isMessageType8(msg, type) {
    return msg.type === type;
  }
  function handleInfoResult(msg) {
    const p = pendingInfo2.get(msg.id);
    if (!p) return;
    pendingInfo2.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.info !== void 0) {
      p.resolve(msg.info);
      return;
    }
    p.reject(new Error(msg.error ?? "upload info unavailable"));
  }
  function handleUploadResult(msg) {
    const p = pendingUpload.get(msg.id);
    if (!p) return;
    pendingUpload.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.result !== void 0) {
      p.resolve(msg.result);
      return;
    }
    p.reject(new Error(msg.error ?? "upload failed"));
  }
  function handleStatusResult(msg) {
    const p = pendingStatus.get(msg.id);
    if (!p) return;
    pendingStatus.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.status !== void 0) {
      p.resolve(msg.status);
      return;
    }
    p.reject(new Error(msg.error ?? "upload status unavailable"));
  }
  function handleStatusChanged(msg) {
    if (!msg.status) return;
    for (const cb of statusHandlers) cb(msg.status);
  }
  function handleUploadMessage(msg) {
    if (isMessageType8(msg, "upload.info.result")) {
      handleInfoResult(msg);
    } else if (isMessageType8(msg, "upload.upload.result")) {
      handleUploadResult(msg);
    } else if (isMessageType8(msg, "upload.status.result")) {
      handleStatusResult(msg);
    } else if (isMessageType8(msg, "upload.status.changed")) {
      handleStatusChanged(msg);
    }
  }
  function info2() {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingInfo2.delete(id)) reject(new Error("upload.info timed out"));
      }, REQUEST_TIMEOUT_MS10);
      pendingInfo2.set(id, { resolve, reject, timeout });
      const msg = {
        type: "upload.info",
        id
      };
      postToShell(msg);
    });
  }
  function upload(request7) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingUpload.delete(id)) reject(new Error("upload.upload timed out"));
      }, REQUEST_TIMEOUT_MS10);
      pendingUpload.set(id, { resolve, reject, timeout });
      const msg = {
        type: "upload.upload",
        id,
        request: request7
      };
      postToShell(msg);
    });
  }
  function status(uploadId) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingStatus.delete(id)) reject(new Error("upload.status timed out"));
      }, REQUEST_TIMEOUT_MS10);
      pendingStatus.set(id, { resolve, reject, timeout });
      const msg = {
        type: "upload.status",
        id,
        uploadId
      };
      postToShell(msg);
    });
  }
  function onStatus(handler) {
    statusHandlers.add(handler);
    return {
      close() {
        statusHandlers.delete(handler);
      }
    };
  }
  function installUploadShim() {
    if (installed10) {
      return () => void 0;
    }
    installed10 = true;
    return () => {
      for (const p of pendingInfo2.values()) clearTimeout(p.timeout);
      for (const p of pendingUpload.values()) clearTimeout(p.timeout);
      for (const p of pendingStatus.values()) clearTimeout(p.timeout);
      pendingInfo2.clear();
      pendingUpload.clear();
      pendingStatus.clear();
      statusHandlers.clear();
      installed10 = false;
    };
  }

  // ../nap/dist/chunk-V2NIFD4E.js
  var REQUEST_TIMEOUT_MS11 = 3e4;
  var pendingInvoke = /* @__PURE__ */ new Map();
  var pendingAvailable = /* @__PURE__ */ new Map();
  var pendingHandlers = /* @__PURE__ */ new Map();
  var changedHandlers = /* @__PURE__ */ new Set();
  var installed11 = false;
  function isMessageType9(msg, type) {
    return msg.type === type;
  }
  function isIntentResult(value) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
    const result = value;
    return typeof result.ok === "boolean" && typeof result.archetype === "string" && typeof result.action === "string" && typeof result.handled === "boolean";
  }
  function handleInvokeResult(msg) {
    const pending4 = pendingInvoke.get(msg.id);
    if (!pending4) return;
    pendingInvoke.delete(msg.id);
    clearTimeout(pending4.timeout);
    if (isIntentResult(msg.result)) {
      pending4.resolve(msg.result);
      return;
    }
    pending4.reject(new Error(msg.error ?? "invalid intent.invoke.result"));
  }
  function handleAvailableResult(msg) {
    const pending4 = pendingAvailable.get(msg.id);
    if (!pending4) return;
    pendingAvailable.delete(msg.id);
    clearTimeout(pending4.timeout);
    if (msg.availability !== void 0) {
      pending4.resolve(msg.availability);
      return;
    }
    pending4.reject(new Error(msg.error ?? "intent availability unavailable"));
  }
  function handleHandlersResult(msg) {
    const pending4 = pendingHandlers.get(msg.id);
    if (!pending4) return;
    pendingHandlers.delete(msg.id);
    clearTimeout(pending4.timeout);
    if (msg.handlers !== void 0) {
      pending4.resolve(msg.handlers);
      return;
    }
    pending4.reject(new Error(msg.error ?? "intent handlers unavailable"));
  }
  function handleChanged(msg) {
    if (!msg.availability) return;
    for (const callback of changedHandlers) callback(msg.availability);
  }
  function handleIntentMessage(msg) {
    if (isMessageType9(msg, "intent.invoke.result")) {
      handleInvokeResult(msg);
    } else if (isMessageType9(msg, "intent.available.result")) {
      handleAvailableResult(msg);
    } else if (isMessageType9(msg, "intent.handlers.result")) {
      handleHandlersResult(msg);
    } else if (isMessageType9(msg, "intent.changed")) {
      handleChanged(msg);
    }
  }
  function invoke(request7) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingInvoke.delete(id)) reject(new Error("intent.invoke timed out"));
      }, REQUEST_TIMEOUT_MS11);
      pendingInvoke.set(id, { resolve, reject, timeout });
      const msg = {
        type: "intent.invoke",
        id,
        request: request7
      };
      postToShell(msg);
    });
  }
  function open2(archetype, payload, opts) {
    return invoke({ archetype, action: "open", payload, ...opts });
  }
  function available(archetype) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingAvailable.delete(id)) reject(new Error("intent.available timed out"));
      }, REQUEST_TIMEOUT_MS11);
      pendingAvailable.set(id, { resolve, reject, timeout });
      const msg = {
        type: "intent.available",
        id,
        archetype
      };
      postToShell(msg);
    });
  }
  function handlers() {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingHandlers.delete(id)) reject(new Error("intent.handlers timed out"));
      }, REQUEST_TIMEOUT_MS11);
      pendingHandlers.set(id, { resolve, reject, timeout });
      const msg = {
        type: "intent.handlers",
        id
      };
      postToShell(msg);
    });
  }
  function onChanged3(handler) {
    changedHandlers.add(handler);
    return {
      close() {
        changedHandlers.delete(handler);
      }
    };
  }
  function installIntentShim() {
    if (installed11) return () => void 0;
    installed11 = true;
    return () => {
      for (const pending4 of pendingInvoke.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingAvailable.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingHandlers.values()) clearTimeout(pending4.timeout);
      pendingInvoke.clear();
      pendingAvailable.clear();
      pendingHandlers.clear();
      changedHandlers.clear();
      installed11 = false;
    };
  }

  // ../nap/dist/chunk-5QWBBZC6.js
  var REQUEST_TIMEOUT_MS12 = 3e4;
  var pending = /* @__PURE__ */ new Map();
  var eventHandlers2 = /* @__PURE__ */ new Set();
  var installed12 = false;
  var WEBRTC_MESSAGE_TYPES = /* @__PURE__ */ new Set([
    "webrtc.open",
    "webrtc.open.result",
    "webrtc.send",
    "webrtc.send.result",
    "webrtc.close",
    "webrtc.close.result",
    "webrtc.event"
  ]);
  function isWebrtcNapMessage(msg) {
    return WEBRTC_MESSAGE_TYPES.has(msg.type);
  }
  function request2(msg, resultType, project) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pending.delete(msg.id)) reject(new Error(`${msg.type} timed out`));
      }, REQUEST_TIMEOUT_MS12);
      pending.set(msg.id, {
        resolve,
        reject,
        timeout,
        resultType,
        project
      });
      postToShell(msg);
    });
  }
  function rejectResult(msg, fallback) {
    if ("error" in msg && msg.error) throw new Error(msg.error);
    throw new Error(fallback);
  }
  function handleEvent2(event) {
    for (const handler of eventHandlers2) handler(event);
  }
  function handleWebrtcMessage(msg) {
    if (!isWebrtcNapMessage(msg)) return;
    if (msg.type === "webrtc.event") {
      handleEvent2(msg.event);
      return;
    }
    if (!("id" in msg) || typeof msg.id !== "string") return;
    const entry = pending.get(msg.id);
    if (!entry || msg.type !== entry.resultType) return;
    pending.delete(msg.id);
    clearTimeout(entry.timeout);
    try {
      entry.resolve(entry.project(msg));
    } catch (error) {
      entry.reject(error instanceof Error ? error : new Error(String(error)));
    }
  }
  function open3(openRequest) {
    return request2(
      { type: "webrtc.open", id: crypto.randomUUID(), request: openRequest },
      "webrtc.open.result",
      (msg) => {
        if (msg.type === "webrtc.open.result" && msg.session) return { session: msg.session };
        rejectResult(msg, "webrtc open failed");
      }
    );
  }
  function send2(sessionId, payload) {
    return request2(
      { type: "webrtc.send", id: crypto.randomUUID(), sessionId, payload },
      "webrtc.send.result",
      (msg) => {
        if (msg.type === "webrtc.send.result" && !msg.error) return void 0;
        rejectResult(msg, "webrtc send failed");
      }
    );
  }
  function close2(sessionId, reason) {
    return request2(
      { type: "webrtc.close", id: crypto.randomUUID(), sessionId, ...reason ? { reason } : {} },
      "webrtc.close.result",
      (msg) => {
        if (msg.type === "webrtc.close.result" && !msg.error) return void 0;
        rejectResult(msg, "webrtc close failed");
      }
    );
  }
  function onEvent2(handler) {
    eventHandlers2.add(handler);
    return {
      close() {
        eventHandlers2.delete(handler);
      }
    };
  }
  function installWebrtcShim() {
    if (installed12) return () => void 0;
    installed12 = true;
    return () => {
      for (const entry of pending.values()) clearTimeout(entry.timeout);
      pending.clear();
      eventHandlers2.clear();
      installed12 = false;
    };
  }

  // ../nap/dist/chunk-23C42MT6.js
  var REQUEST_TIMEOUT_MS13 = 3e4;
  var pending2 = /* @__PURE__ */ new Map();
  var eventHandlers3 = /* @__PURE__ */ new Set();
  var installed13 = false;
  var BLE_MESSAGE_TYPES = /* @__PURE__ */ new Set([
    "ble.open",
    "ble.open.result",
    "ble.services",
    "ble.services.result",
    "ble.read",
    "ble.read.result",
    "ble.write",
    "ble.write.result",
    "ble.subscribe",
    "ble.subscribe.result",
    "ble.unsubscribe",
    "ble.unsubscribe.result",
    "ble.close",
    "ble.close.result",
    "ble.event"
  ]);
  function isBleNapMessage(msg) {
    return BLE_MESSAGE_TYPES.has(msg.type);
  }
  function request3(msg, resultType, project) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pending2.delete(msg.id)) reject(new Error(`${msg.type} timed out`));
      }, REQUEST_TIMEOUT_MS13);
      pending2.set(msg.id, {
        resolve,
        reject,
        timeout,
        resultType,
        project
      });
      postToShell(msg);
    });
  }
  function rejectResult2(msg, fallback) {
    if ("error" in msg && msg.error) throw new Error(msg.error);
    throw new Error(fallback);
  }
  function handleEvent3(event) {
    for (const handler of eventHandlers3) handler(event);
  }
  function handleBleMessage(msg) {
    if (!isBleNapMessage(msg)) return;
    if (msg.type === "ble.event") {
      handleEvent3(msg.event);
      return;
    }
    if (!("id" in msg) || typeof msg.id !== "string") return;
    const entry = pending2.get(msg.id);
    if (!entry || msg.type !== entry.resultType) return;
    pending2.delete(msg.id);
    clearTimeout(entry.timeout);
    try {
      entry.resolve(entry.project(msg));
    } catch (error) {
      entry.reject(error instanceof Error ? error : new Error(String(error)));
    }
  }
  function open4(openRequest) {
    return request3(
      { type: "ble.open", id: crypto.randomUUID(), request: openRequest },
      "ble.open.result",
      (msg) => {
        if (msg.type === "ble.open.result" && msg.session) return { session: msg.session };
        rejectResult2(msg, "ble open failed");
      }
    );
  }
  function services(sessionId) {
    return request3(
      { type: "ble.services", id: crypto.randomUUID(), sessionId },
      "ble.services.result",
      (msg) => {
        if (msg.type === "ble.services.result" && msg.services !== void 0) return msg.services;
        rejectResult2(msg, "ble services unavailable");
      }
    );
  }
  function read(sessionId, target) {
    return request3(
      { type: "ble.read", id: crypto.randomUUID(), sessionId, target },
      "ble.read.result",
      (msg) => {
        if (msg.type === "ble.read.result" && msg.data !== void 0) return msg.data;
        rejectResult2(msg, "ble read failed");
      }
    );
  }
  function write(sessionId, target, data, options) {
    return request3(
      { type: "ble.write", id: crypto.randomUUID(), sessionId, target, data, ...options ? { options } : {} },
      "ble.write.result",
      (msg) => {
        if (msg.type === "ble.write.result" && !msg.error) return void 0;
        rejectResult2(msg, "ble write failed");
      }
    );
  }
  function subscribe3(sessionId, target) {
    return request3(
      { type: "ble.subscribe", id: crypto.randomUUID(), sessionId, target },
      "ble.subscribe.result",
      (msg) => {
        if (msg.type === "ble.subscribe.result" && !msg.error) return void 0;
        rejectResult2(msg, "ble subscribe failed");
      }
    );
  }
  function unsubscribe(sessionId, target) {
    return request3(
      { type: "ble.unsubscribe", id: crypto.randomUUID(), sessionId, target },
      "ble.unsubscribe.result",
      (msg) => {
        if (msg.type === "ble.unsubscribe.result" && !msg.error) return void 0;
        rejectResult2(msg, "ble unsubscribe failed");
      }
    );
  }
  function close3(sessionId, reason) {
    return request3(
      { type: "ble.close", id: crypto.randomUUID(), sessionId, ...reason ? { reason } : {} },
      "ble.close.result",
      (msg) => {
        if (msg.type === "ble.close.result" && !msg.error) return void 0;
        rejectResult2(msg, "ble close failed");
      }
    );
  }
  function onEvent3(handler) {
    eventHandlers3.add(handler);
    return {
      close() {
        eventHandlers3.delete(handler);
      }
    };
  }
  function installBleShim() {
    if (installed13) return () => void 0;
    installed13 = true;
    return () => {
      for (const entry of pending2.values()) clearTimeout(entry.timeout);
      pending2.clear();
      eventHandlers3.clear();
      installed13 = false;
    };
  }

  // ../nap/dist/chunk-A6WF7BXA.js
  var REQUEST_TIMEOUT_MS14 = 3e4;
  var pendingOpen2 = /* @__PURE__ */ new Map();
  var installed14 = false;
  function isMessageType10(msg, type) {
    return msg.type === type;
  }
  function handleOpenResult2(msg) {
    const p = pendingOpen2.get(msg.id);
    if (!p) return;
    pendingOpen2.delete(msg.id);
    clearTimeout(p.timeout);
    if (msg.status === "opened" || msg.status === "denied") {
      p.resolve({ status: msg.status });
      return;
    }
    p.reject(new Error(msg.error ?? "link open failed"));
  }
  function handleLinkMessage(msg) {
    if (isMessageType10(msg, "link.open.result")) {
      handleOpenResult2(msg);
    }
  }
  function open5(url, options) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingOpen2.delete(id)) reject(new Error("link.open timed out"));
      }, REQUEST_TIMEOUT_MS14);
      pendingOpen2.set(id, { resolve, reject, timeout });
      const msg = {
        type: "link.open",
        id,
        url,
        ...options ? { options } : {}
      };
      postToShell(msg);
    });
  }
  function installLinkShim() {
    if (installed14) {
      return () => void 0;
    }
    installed14 = true;
    return () => {
      for (const p of pendingOpen2.values()) {
        clearTimeout(p.timeout);
        p.reject(new Error("link shim uninstalled"));
      }
      pendingOpen2.clear();
      installed14 = false;
    };
  }

  // ../nap/dist/chunk-HNGITPFS.js
  var REQUEST_TIMEOUT_MS15 = 3e4;
  var pendingQuery2 = /* @__PURE__ */ new Map();
  var installed15 = false;
  function isMessageType11(msg, type) {
    return msg.type === type;
  }
  function normalizeFilters(filters) {
    return Array.isArray(filters) ? filters : [filters];
  }
  function handleQueryResult2(msg) {
    const pending4 = pendingQuery2.get(msg.id);
    if (!pending4) return;
    pendingQuery2.delete(msg.id);
    clearTimeout(pending4.timeout);
    pending4.resolve({
      ok: msg.ok,
      ...msg.count === void 0 ? {} : { count: msg.count },
      ...msg.approximate === void 0 ? {} : { approximate: msg.approximate },
      ...msg.hll === void 0 ? {} : { hll: msg.hll },
      ...msg.relays === void 0 ? {} : { relays: msg.relays },
      ...msg.error === void 0 ? {} : { error: msg.error },
      ...msg.reason === void 0 ? {} : { reason: msg.reason }
    });
  }
  function handleCountMessage(msg) {
    if (isMessageType11(msg, "count.query.result")) {
      handleQueryResult2(msg);
    }
  }
  function query2(filters, options) {
    const normalizedFilters = normalizeFilters(filters);
    if (normalizedFilters.length === 0) {
      return Promise.reject(new Error("count.query requires at least one filter"));
    }
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingQuery2.delete(id)) reject(new Error("count.query timed out"));
      }, REQUEST_TIMEOUT_MS15);
      pendingQuery2.set(id, { resolve, reject, timeout });
      const msg = {
        type: "count.query",
        id,
        filters: normalizedFilters,
        ...options ? { options } : {}
      };
      postToShell(msg);
    });
  }
  function installCountShim() {
    if (installed15) {
      return () => void 0;
    }
    installed15 = true;
    return () => {
      for (const pending4 of pendingQuery2.values()) clearTimeout(pending4.timeout);
      pendingQuery2.clear();
      installed15 = false;
    };
  }

  // ../nap/dist/chunk-ESEMRW4T.js
  var REQUEST_TIMEOUT_MS16 = 3e4;
  var pendingSupported = /* @__PURE__ */ new Map();
  var pendingAdd = /* @__PURE__ */ new Map();
  var pendingRemove = /* @__PURE__ */ new Map();
  var installed16 = false;
  function isMessageType12(msg, type) {
    return msg.type === type;
  }
  function settle(pending4, id, resolveValue, fallbackError, error) {
    const p = pending4.get(id);
    if (!p) return;
    pending4.delete(id);
    clearTimeout(p.timeout);
    if (resolveValue !== void 0) {
      p.resolve(resolveValue);
      return;
    }
    p.reject(new Error(error ?? fallbackError));
  }
  function mutationResultBaseFrom(msg) {
    const result = { ok: msg.ok };
    if (msg.eventId !== void 0) result.eventId = msg.eventId;
    if (msg.event !== void 0) result.event = msg.event;
    if (msg.skipped !== void 0) result.skipped = msg.skipped;
    if (msg.error !== void 0) result.error = msg.error;
    if (msg.reason !== void 0) result.reason = msg.reason;
    if (msg.supported !== void 0) result.supported = msg.supported;
    return result;
  }
  function addResultFrom(msg) {
    const result = mutationResultBaseFrom(msg);
    if (msg.added !== void 0) result.added = msg.added;
    return result;
  }
  function removeResultFrom(msg) {
    const result = mutationResultBaseFrom(msg);
    if (msg.removed !== void 0) result.removed = msg.removed;
    return result;
  }
  function handleSupportedResult(msg) {
    settle(
      pendingSupported,
      msg.id,
      msg.lists,
      "lists.supported failed",
      msg.error
    );
  }
  function handleAddResult(msg) {
    settle(pendingAdd, msg.id, addResultFrom(msg), "lists.add failed");
  }
  function handleRemoveResult(msg) {
    settle(pendingRemove, msg.id, removeResultFrom(msg), "lists.remove failed");
  }
  function handleListsMessage(msg) {
    if (isMessageType12(msg, "lists.supported.result")) {
      handleSupportedResult(msg);
    } else if (isMessageType12(msg, "lists.add.result")) {
      handleAddResult(msg);
    } else if (isMessageType12(msg, "lists.remove.result")) {
      handleRemoveResult(msg);
    }
  }
  function request4(type, pending4, payload) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pending4.delete(id)) reject(new Error(`${type} timed out`));
      }, REQUEST_TIMEOUT_MS16);
      pending4.set(id, { resolve, reject, timeout });
      postToShell({
        type,
        id,
        ...payload
      });
    });
  }
  function supported() {
    return request4("lists.supported", pendingSupported, {});
  }
  function add(list3, items, options) {
    return request4("lists.add", pendingAdd, {
      list: list3,
      items,
      ...options ? { options } : {}
    });
  }
  function remove(list3, items, options) {
    return request4("lists.remove", pendingRemove, {
      list: list3,
      items,
      ...options ? { options } : {}
    });
  }
  function installListsShim() {
    if (installed16) {
      return () => void 0;
    }
    installed16 = true;
    return () => {
      for (const p of pendingSupported.values()) {
        clearTimeout(p.timeout);
        p.reject(new Error("lists shim uninstalled"));
      }
      for (const p of pendingAdd.values()) {
        clearTimeout(p.timeout);
        p.reject(new Error("lists shim uninstalled"));
      }
      for (const p of pendingRemove.values()) {
        clearTimeout(p.timeout);
        p.reject(new Error("lists shim uninstalled"));
      }
      pendingSupported.clear();
      pendingAdd.clear();
      pendingRemove.clear();
      installed16 = false;
    };
  }

  // ../nap/dist/chunk-EALGKE7J.js
  var REQUEST_TIMEOUT_MS17 = 3e4;
  var pendingEncode = /* @__PURE__ */ new Map();
  var pendingDecode = /* @__PURE__ */ new Map();
  var pendingProfile = /* @__PURE__ */ new Map();
  var pendingFollows = /* @__PURE__ */ new Map();
  var pendingFollow = /* @__PURE__ */ new Map();
  var pendingUnfollow = /* @__PURE__ */ new Map();
  var pendingReact = /* @__PURE__ */ new Map();
  var pendingReport = /* @__PURE__ */ new Map();
  var installed17 = false;
  function isMessageType13(msg, type) {
    return msg.type === type;
  }
  function withoutEnvelope(msg) {
    const { type: _type, id: _id, ...result } = msg;
    return result;
  }
  function resolveResult(pending4, msg, fallback) {
    const p = pending4.get(msg.id);
    if (!p) return;
    pending4.delete(msg.id);
    clearTimeout(p.timeout);
    if (typeof msg.ok === "boolean") {
      p.resolve(withoutEnvelope(msg));
      return;
    }
    p.reject(new Error(msg.error ?? fallback));
  }
  function request5(pending4, timeoutMessage, message) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pending4.delete(id)) reject(new Error(timeoutMessage));
      }, REQUEST_TIMEOUT_MS17);
      pending4.set(id, { resolve, reject, timeout });
      postToShell(message(id));
    });
  }
  function handleCommonMessage(msg) {
    if (isMessageType13(msg, "common.encodeNip19.result")) {
      resolveResult(pendingEncode, msg, "common.encodeNip19 failed");
    } else if (isMessageType13(msg, "common.decodeNip19.result")) {
      resolveResult(pendingDecode, msg, "common.decodeNip19 failed");
    } else if (isMessageType13(msg, "common.getProfile.result")) {
      resolveResult(pendingProfile, msg, "common.getProfile failed");
    } else if (isMessageType13(msg, "common.follows.result")) {
      resolveResult(pendingFollows, msg, "common.follows failed");
    } else if (isMessageType13(msg, "common.follow.result")) {
      resolveResult(pendingFollow, msg, "common.follow failed");
    } else if (isMessageType13(msg, "common.unfollow.result")) {
      resolveResult(pendingUnfollow, msg, "common.unfollow failed");
    } else if (isMessageType13(msg, "common.react.result")) {
      resolveResult(pendingReact, msg, "common.react failed");
    } else if (isMessageType13(msg, "common.report.result")) {
      resolveResult(pendingReport, msg, "common.report failed");
    }
  }
  function encodeNip19(input) {
    return request5(pendingEncode, "common.encodeNip19 timed out", (id) => ({
      type: "common.encodeNip19",
      id,
      input
    }));
  }
  function decodeNip19(value) {
    return request5(pendingDecode, "common.decodeNip19 timed out", (id) => ({
      type: "common.decodeNip19",
      id,
      value
    }));
  }
  function getProfile2(target) {
    return request5(pendingProfile, "common.getProfile timed out", (id) => ({
      type: "common.getProfile",
      id,
      target
    }));
  }
  function follows() {
    return request5(pendingFollows, "common.follows timed out", (id) => ({
      type: "common.follows",
      id
    }));
  }
  function follow(...pubkeys) {
    return request5(pendingFollow, "common.follow timed out", (id) => ({
      type: "common.follow",
      id,
      pubkeys
    }));
  }
  function unfollow(...pubkeys) {
    return request5(pendingUnfollow, "common.unfollow timed out", (id) => ({
      type: "common.unfollow",
      id,
      pubkeys
    }));
  }
  function react(targetEventId, reaction, customEmojiHref) {
    return request5(pendingReact, "common.react timed out", (id) => ({
      type: "common.react",
      id,
      targetEventId,
      reaction,
      ...customEmojiHref === void 0 ? {} : { customEmojiHref }
    }));
  }
  function report(target, reason, text) {
    return request5(pendingReport, "common.report timed out", (id) => ({
      type: "common.report",
      id,
      target,
      reason,
      text
    }));
  }
  function installCommonShim() {
    if (installed17) {
      return () => void 0;
    }
    installed17 = true;
    return () => {
      for (const pending4 of [
        pendingEncode,
        pendingDecode,
        pendingProfile,
        pendingFollows,
        pendingFollow,
        pendingUnfollow,
        pendingReact,
        pendingReport
      ]) {
        for (const p of pending4.values()) clearTimeout(p.timeout);
        pending4.clear();
      }
      installed17 = false;
    };
  }

  // ../nap/dist/chunk-6GWPHINR.js
  var REQUEST_TIMEOUT_MS18 = 3e4;
  var pendingOpen3 = /* @__PURE__ */ new Map();
  var pendingWrite = /* @__PURE__ */ new Map();
  var pendingClose2 = /* @__PURE__ */ new Map();
  var eventHandlers4 = /* @__PURE__ */ new Set();
  var installed18 = false;
  function isMessageType14(msg, type) {
    return msg.type === type;
  }
  function handleOpenResult3(msg) {
    const pending4 = pendingOpen3.get(msg.id);
    if (!pending4) return;
    pendingOpen3.delete(msg.id);
    clearTimeout(pending4.timeout);
    if (msg.session !== void 0) {
      pending4.resolve({ session: msg.session });
      return;
    }
    pending4.reject(new Error(msg.error ?? "serial open failed"));
  }
  function handleWriteResult(msg) {
    const pending4 = pendingWrite.get(msg.id);
    if (!pending4) return;
    pendingWrite.delete(msg.id);
    clearTimeout(pending4.timeout);
    if (msg.error) {
      pending4.reject(new Error(msg.error));
      return;
    }
    pending4.resolve();
  }
  function handleCloseResult2(msg) {
    const pending4 = pendingClose2.get(msg.id);
    if (!pending4) return;
    pendingClose2.delete(msg.id);
    clearTimeout(pending4.timeout);
    if (msg.error) {
      pending4.reject(new Error(msg.error));
      return;
    }
    pending4.resolve();
  }
  function handleEvent4(msg) {
    if (!msg.event) return;
    for (const handler of eventHandlers4) handler(msg.event);
  }
  function handleSerialMessage(msg) {
    if (isMessageType14(msg, "serial.open.result")) {
      handleOpenResult3(msg);
    } else if (isMessageType14(msg, "serial.write.result")) {
      handleWriteResult(msg);
    } else if (isMessageType14(msg, "serial.close.result")) {
      handleCloseResult2(msg);
    } else if (isMessageType14(msg, "serial.event")) {
      handleEvent4(msg);
    }
  }
  function open6(request7) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingOpen3.delete(id)) reject(new Error("serial.open timed out"));
      }, REQUEST_TIMEOUT_MS18);
      pendingOpen3.set(id, { resolve, reject, timeout });
      const msg = {
        type: "serial.open",
        id,
        request: request7
      };
      postToShell(msg);
    });
  }
  function write2(sessionId, data) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingWrite.delete(id)) reject(new Error("serial.write timed out"));
      }, REQUEST_TIMEOUT_MS18);
      pendingWrite.set(id, { resolve, reject, timeout });
      const msg = {
        type: "serial.write",
        id,
        sessionId,
        data: Array.from(data)
      };
      postToShell(msg);
    });
  }
  function close4(sessionId, reason) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingClose2.delete(id)) reject(new Error("serial.close timed out"));
      }, REQUEST_TIMEOUT_MS18);
      pendingClose2.set(id, { resolve, reject, timeout });
      const msg = {
        type: "serial.close",
        id,
        sessionId,
        ...reason === void 0 ? {} : { reason }
      };
      postToShell(msg);
    });
  }
  function onEvent4(handler) {
    eventHandlers4.add(handler);
    return {
      close() {
        eventHandlers4.delete(handler);
      }
    };
  }
  function installSerialShim() {
    if (installed18) {
      return () => void 0;
    }
    installed18 = true;
    return () => {
      for (const pending4 of pendingOpen3.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingWrite.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingClose2.values()) clearTimeout(pending4.timeout);
      pendingOpen3.clear();
      pendingWrite.clear();
      pendingClose2.clear();
      eventHandlers4.clear();
      installed18 = false;
    };
  }

  // ../nap/dist/chunk-PONXETIR.js
  var REQUEST_TIMEOUT_MS19 = 3e4;
  var pending3 = /* @__PURE__ */ new Map();
  var changeHandlers3 = /* @__PURE__ */ new Set();
  var installed19 = false;
  var RESULT_TYPES = /* @__PURE__ */ new Set([
    "fs.info.result",
    "fs.pickFile.result",
    "fs.pickFiles.result",
    "fs.pickDirectory.result",
    "fs.pickSaveFile.result",
    "fs.stat.result",
    "fs.list.result",
    "fs.read.result",
    "fs.write.result",
    "fs.mkdir.result",
    "fs.remove.result",
    "fs.move.result",
    "fs.watch.result",
    "fs.unwatch.result"
  ]);
  function request6(type, payload = {}) {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pending3.delete(id)) reject(new Error(`${type} timed out`));
      }, REQUEST_TIMEOUT_MS19);
      pending3.set(id, { resolve, reject, timeout });
      postToShell({ type, id, ...payload });
    });
  }
  function expectField(msg, field, operation) {
    const value = msg[field];
    if (value === void 0) {
      throw new Error(`${operation} returned no ${field}`);
    }
    return value;
  }
  function handleResult(msg) {
    const id = msg.id;
    if (typeof id !== "string") return;
    const entry = pending3.get(id);
    if (!entry) return;
    pending3.delete(id);
    clearTimeout(entry.timeout);
    if (typeof msg.error === "string") {
      entry.reject(new Error(msg.error));
      return;
    }
    entry.resolve(msg);
  }
  function handleChanged2(msg) {
    const change = msg.change;
    if (!change) return;
    for (const handler of changeHandlers3) handler(change);
  }
  function handleFsMessage(msg) {
    if (RESULT_TYPES.has(msg.type)) {
      handleResult(msg);
    } else if (msg.type === "fs.changed") {
      handleChanged2(msg);
    }
  }
  async function info3() {
    const msg = await request6("fs.info");
    return expectField(msg, "info", "fs.info");
  }
  async function pickFile(options) {
    const msg = await request6("fs.pickFile", options === void 0 ? {} : { options });
    return expectField(msg, "result", "fs.pickFile");
  }
  async function pickFiles(options) {
    const msg = await request6("fs.pickFiles", options === void 0 ? {} : { options });
    return expectField(msg, "result", "fs.pickFiles");
  }
  async function pickDirectory(options) {
    const msg = await request6("fs.pickDirectory", options === void 0 ? {} : { options });
    return expectField(msg, "result", "fs.pickDirectory");
  }
  async function pickSaveFile(options) {
    const msg = await request6("fs.pickSaveFile", options === void 0 ? {} : { options });
    return expectField(msg, "result", "fs.pickSaveFile");
  }
  async function stat(path) {
    const msg = await request6("fs.stat", { path });
    return expectField(msg, "metadata", "fs.stat");
  }
  async function list2(path) {
    const msg = await request6("fs.list", { path });
    return expectField(msg, "entries", "fs.list");
  }
  async function read2(path, options) {
    const msg = await request6("fs.read", { path, ...options === void 0 ? {} : { options } });
    return expectField(msg, "result", "fs.read");
  }
  async function write3(path, data, options) {
    const msg = await request6("fs.write", {
      path,
      data,
      ...options === void 0 ? {} : { options }
    });
    return expectField(msg, "result", "fs.write");
  }
  async function mkdir(path, options) {
    await request6("fs.mkdir", { path, ...options === void 0 ? {} : { options } });
  }
  async function remove2(path, recursive) {
    await request6("fs.remove", { path, ...recursive === void 0 ? {} : { recursive } });
  }
  async function move(fromPath, toPath) {
    await request6("fs.move", { fromPath, toPath });
  }
  async function watch(path, options) {
    const msg = await request6("fs.watch", { path, ...options === void 0 ? {} : { options } });
    return expectField(msg, "watchId", "fs.watch");
  }
  async function unwatch(watchId) {
    await request6("fs.unwatch", { watchId });
  }
  function onChanged4(handler) {
    changeHandlers3.add(handler);
    return {
      close() {
        changeHandlers3.delete(handler);
      }
    };
  }
  function installFsShim() {
    if (installed19) {
      return () => void 0;
    }
    installed19 = true;
    return () => {
      for (const entry of pending3.values()) clearTimeout(entry.timeout);
      pending3.clear();
      changeHandlers3.clear();
      installed19 = false;
    };
  }

  // ../nap/dist/chunk-DNUVIJ5U.js
  var REQUEST_TIMEOUT_MS20 = 3e4;
  var pendingStatus2 = /* @__PURE__ */ new Map();
  var pendingConversations = /* @__PURE__ */ new Map();
  var pendingMessages = /* @__PURE__ */ new Map();
  var pendingSend = /* @__PURE__ */ new Map();
  var pendingSubscribe = /* @__PURE__ */ new Map();
  var pendingUnsubscribe = /* @__PURE__ */ new Map();
  var messageHandlers = /* @__PURE__ */ new Set();
  var installed20 = false;
  function isMessageType15(msg, type) {
    return msg.type === type;
  }
  function createPending(map, id, action) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (map.delete(id)) reject(new Error(`${action} timed out`));
      }, REQUEST_TIMEOUT_MS20);
      map.set(id, { resolve, reject, timeout });
    });
  }
  function settle2(map, id, error, fallbackError, getValue) {
    const pending4 = map.get(id);
    if (!pending4) return;
    map.delete(id);
    clearTimeout(pending4.timeout);
    if (error) {
      pending4.reject(new Error(error));
      return;
    }
    const value = getValue();
    if (value === void 0) {
      pending4.reject(new Error(fallbackError));
      return;
    }
    pending4.resolve(value);
  }
  function handleStatusResult2(msg) {
    settle2(
      pendingStatus2,
      msg.id,
      msg.error,
      "dm.status.result missing status",
      () => {
        if (typeof msg.available !== "boolean" || !Array.isArray(msg.implementations) || !Array.isArray(msg.capabilities)) {
          return void 0;
        }
        return {
          available: msg.available,
          ...msg.ownerPubkey === void 0 ? {} : { ownerPubkey: msg.ownerPubkey },
          implementations: msg.implementations,
          capabilities: msg.capabilities
        };
      }
    );
  }
  function handleConversationsResult(msg) {
    settle2(
      pendingConversations,
      msg.id,
      msg.error,
      "dm.conversations.result missing conversations",
      () => {
        if (!Array.isArray(msg.conversations)) return void 0;
        return {
          conversations: msg.conversations,
          ...msg.cursor === void 0 ? {} : { cursor: msg.cursor }
        };
      }
    );
  }
  function handleMessagesResult(msg) {
    settle2(
      pendingMessages,
      msg.id,
      msg.error,
      "dm.messages.result missing messages",
      () => {
        if (!Array.isArray(msg.messages)) return void 0;
        return {
          messages: msg.messages,
          ...msg.cursor === void 0 ? {} : { cursor: msg.cursor }
        };
      }
    );
  }
  function handleSendResult2(msg) {
    settle2(
      pendingSend,
      msg.id,
      msg.error,
      "dm.send.result missing message",
      () => {
        if (typeof msg.ok !== "boolean" || msg.message === void 0) return void 0;
        return { ok: msg.ok, message: msg.message };
      }
    );
  }
  function handleSubscribeResult(msg) {
    settle2(
      pendingSubscribe,
      msg.id,
      msg.error,
      "dm.subscribe.result missing subscriptionId",
      () => {
        if (typeof msg.subscriptionId !== "string") return void 0;
        return { subscriptionId: msg.subscriptionId };
      }
    );
  }
  function handleUnsubscribeResult(msg) {
    settle2(
      pendingUnsubscribe,
      msg.id,
      msg.error,
      "dm.unsubscribe.result missing ok",
      () => {
        if (typeof msg.ok !== "boolean") return void 0;
        return { ok: msg.ok };
      }
    );
  }
  function handleMessageEvent(msg) {
    if (!msg.message || typeof msg.subscriptionId !== "string") return;
    for (const handler of messageHandlers) handler(msg.message, msg.subscriptionId);
  }
  function handleDmMessage(msg) {
    if (isMessageType15(msg, "dm.status.result")) {
      handleStatusResult2(msg);
    } else if (isMessageType15(msg, "dm.conversations.result")) {
      handleConversationsResult(msg);
    } else if (isMessageType15(msg, "dm.messages.result")) {
      handleMessagesResult(msg);
    } else if (isMessageType15(msg, "dm.send.result")) {
      handleSendResult2(msg);
    } else if (isMessageType15(msg, "dm.subscribe.result")) {
      handleSubscribeResult(msg);
    } else if (isMessageType15(msg, "dm.unsubscribe.result")) {
      handleUnsubscribeResult(msg);
    } else if (isMessageType15(msg, "dm.message")) {
      handleMessageEvent(msg);
    }
  }
  function status2() {
    const id = crypto.randomUUID();
    const promise = createPending(pendingStatus2, id, "dm.status");
    const msg = { type: "dm.status", id };
    postToShell(msg);
    return promise;
  }
  function conversations(query4 = {}) {
    const id = crypto.randomUUID();
    const promise = createPending(pendingConversations, id, "dm.conversations");
    const msg = {
      type: "dm.conversations",
      id,
      ...query4
    };
    postToShell(msg);
    return promise;
  }
  function messages(query4) {
    const id = crypto.randomUUID();
    const promise = createPending(pendingMessages, id, "dm.messages");
    const msg = {
      type: "dm.messages",
      id,
      ...query4
    };
    postToShell(msg);
    return promise;
  }
  function send3(request7) {
    const id = crypto.randomUUID();
    const promise = createPending(pendingSend, id, "dm.send");
    const msg = {
      type: "dm.send",
      id,
      ...request7
    };
    postToShell(msg);
    return promise;
  }
  function subscribe4(request7 = {}) {
    const id = crypto.randomUUID();
    const promise = createPending(pendingSubscribe, id, "dm.subscribe");
    const msg = {
      type: "dm.subscribe",
      id,
      ...request7
    };
    postToShell(msg);
    return promise;
  }
  function unsubscribe2(subscriptionId) {
    const id = crypto.randomUUID();
    const promise = createPending(pendingUnsubscribe, id, "dm.unsubscribe");
    const msg = {
      type: "dm.unsubscribe",
      id,
      subscriptionId
    };
    postToShell(msg);
    return promise;
  }
  function onMessage(handler) {
    messageHandlers.add(handler);
    return {
      close() {
        messageHandlers.delete(handler);
      }
    };
  }
  function installDmShim() {
    if (installed20) {
      return () => void 0;
    }
    installed20 = true;
    return () => {
      for (const pending4 of pendingStatus2.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingConversations.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingMessages.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingSend.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingSubscribe.values()) clearTimeout(pending4.timeout);
      for (const pending4 of pendingUnsubscribe.values()) clearTimeout(pending4.timeout);
      pendingStatus2.clear();
      pendingConversations.clear();
      pendingMessages.clear();
      pendingSend.clear();
      pendingSubscribe.clear();
      pendingUnsubscribe.clear();
      messageHandlers.clear();
      installed20 = false;
    };
  }

  // ../nap/dist/chunk-GCEEMD7Y.js
  function subscribe5(filters, onEvent5, onEose, options) {
    const normalizedFilters = Array.isArray(filters) ? filters : [filters];
    const subId = crypto.randomUUID();
    function handleMessage(msgEvent) {
      if (msgEvent.source !== window.parent) return;
      const msg = msgEvent.data;
      if (typeof msg !== "object" || msg === null || typeof msg.type !== "string") return;
      if (!msg.type.startsWith("relay.")) return;
      const typedMsg = msg;
      if (!("subId" in typedMsg) || typedMsg.subId !== subId) return;
      if (msg.type === "relay.event") {
        const eventMsg = msg;
        hydrateResourceCache(eventMsg.result.sidecar?.resources);
        onEvent5(eventMsg.result);
      } else if (msg.type === "relay.eose") {
        onEose();
      } else if (msg.type === "relay.closed") {
        window.removeEventListener("message", handleMessage);
      }
    }
    window.addEventListener("message", handleMessage);
    const subscribeMsg = {
      type: "relay.subscribe",
      id: crypto.randomUUID(),
      subId,
      filters: normalizedFilters,
      ...options?.relay ? { relay: options.relay } : {}
    };
    postToShell(subscribeMsg);
    return {
      close() {
        const closeMsg = {
          type: "relay.close",
          id: crypto.randomUUID(),
          subId
        };
        postToShell(closeMsg);
        window.removeEventListener("message", handleMessage);
      }
    };
  }
  function publish2(template, _options) {
    const publishId = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      function handleMessage(msgEvent) {
        if (msgEvent.source !== window.parent) return;
        const msg = msgEvent.data;
        if (typeof msg !== "object" || msg === null || typeof msg.type !== "string") return;
        if (msg.type !== "relay.publish.result" && msg.type !== "relay.publish.error") return;
        const result = msg;
        if (result.id !== publishId) return;
        window.removeEventListener("message", handleMessage);
        if (result.error || msg.type === "relay.publish.error") {
          reject(new Error(result.error || "relay:write denied"));
        } else if (!result.event) {
          reject(new Error("relay.publish.result missing event"));
        } else {
          resolve(result.event);
        }
      }
      window.addEventListener("message", handleMessage);
      const publishMsg = {
        type: "relay.publish",
        id: publishId,
        event: template
      };
      postToShell(publishMsg);
    });
  }
  function publishEncrypted(template, recipient, encryption = "nip44") {
    const requestId = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      function handleMessage(msgEvent) {
        if (msgEvent.source !== window.parent) return;
        const msg2 = msgEvent.data;
        if (typeof msg2 !== "object" || msg2 === null || typeof msg2.type !== "string") return;
        if (msg2.type !== "relay.publishEncrypted.result") return;
        const result = msg2;
        if (result.id !== requestId) return;
        window.removeEventListener("message", handleMessage);
        if (result.error) {
          reject(new Error(result.error));
        } else if (!result.event) {
          reject(new Error("relay.publishEncrypted.result missing event"));
        } else {
          resolve(result.event);
        }
      }
      window.addEventListener("message", handleMessage);
      const msg = {
        type: "relay.publishEncrypted",
        id: requestId,
        event: template,
        recipient,
        encryption
      };
      postToShell(msg);
    });
  }
  function query3(filters) {
    const normalizedFilters = Array.isArray(filters) ? filters : [filters];
    const queryId = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      function handleMessage(msgEvent) {
        if (msgEvent.source !== window.parent) return;
        const msg = msgEvent.data;
        if (typeof msg !== "object" || msg === null || typeof msg.type !== "string") return;
        if (msg.type !== "relay.query.result") return;
        const result = msg;
        if (result.id !== queryId) return;
        window.removeEventListener("message", handleMessage);
        if (result.error) {
          reject(new Error(result.error));
        } else {
          const events = Array.isArray(result.events) ? result.events : [];
          for (const item of events) hydrateResourceCache(item.sidecar?.resources);
          resolve(events);
        }
      }
      window.addEventListener("message", handleMessage);
      const queryMsg = {
        type: "relay.query",
        id: queryId,
        filters: normalizedFilters
      };
      postToShell(queryMsg);
    });
  }

  // src/runtime-globals-core.ts
  function installCoreDomains(domains, napplet) {
    installFoundationDomains(domains, napplet);
    installPresentationDomains(domains, napplet);
  }
  function installFoundationDomains(domains, napplet) {
    if (domains.has("relay")) {
      napplet.relay = {
        subscribe: subscribe5,
        publish: publish2,
        publishEncrypted,
        query: query3
      };
    }
    if (domains.has("inc")) {
      napplet.inc = {
        emit,
        on,
        channel
      };
    }
    if (domains.has("storage")) {
      napplet.storage = {
        getItem: nappletStorage.getItem.bind(nappletStorage),
        setItem: nappletStorage.setItem.bind(nappletStorage),
        removeItem: nappletStorage.removeItem.bind(nappletStorage),
        keys: nappletStorage.keys.bind(nappletStorage),
        instance: {
          getItem: nappletStorage.instance.getItem.bind(nappletStorage.instance),
          setItem: nappletStorage.instance.setItem.bind(nappletStorage.instance),
          removeItem: nappletStorage.instance.removeItem.bind(nappletStorage.instance),
          keys: nappletStorage.instance.keys.bind(nappletStorage.instance)
        }
      };
    }
    if (domains.has("keys")) {
      napplet.keys = {
        registerAction,
        unregisterAction,
        onAction
      };
    }
    if (domains.has("media")) {
      napplet.media = {
        createSession,
        updateSession,
        destroySession,
        reportState,
        reportCapabilities,
        sendCommand,
        onCommand,
        onState,
        onCapabilities,
        onControls
      };
    }
    if (domains.has("notify")) {
      napplet.notify = {
        send,
        dismiss,
        badge,
        registerChannel,
        requestPermission,
        onAction: onAction2,
        onClicked,
        onDismissed,
        onControls: onControls2
      };
    }
  }
  function installPresentationDomains(domains, napplet) {
    if (domains.has("identity")) {
      napplet.identity = {
        getPublicKey,
        onChanged,
        getRelays,
        getProfile,
        getFollows,
        getList,
        getZaps,
        getMutes,
        getBlocked,
        getBadges
      };
    }
    if (domains.has("theme")) {
      napplet.theme = {
        get,
        onChanged: onChanged2
      };
    }
    if (domains.has("config")) {
      napplet.config = {
        registerSchema,
        get: get2,
        subscribe,
        openSettings,
        onSchemaError,
        schema: null
      };
    }
  }

  // src/runtime-globals-services.ts
  function installServiceDomains(domains, napplet) {
    installNetworkDomains(domains, napplet);
    installDeviceDomains(domains, napplet);
    installFilesystemDomains(domains, napplet);
  }
  function installNetworkDomains(domains, napplet) {
    if (domains.has("resource")) {
      napplet.resource = {
        info,
        bytes,
        bytesMany,
        bytesAsObjectURL
      };
    }
    if (domains.has("cvm")) {
      napplet.cvm = {
        discover,
        request,
        listTools,
        callTool,
        listResources,
        readResource,
        close,
        onEvent,
        registry: {
          list: registryList,
          has: registryHas,
          describe: registryDescribe,
          call: registryCall
        }
      };
    }
    if (domains.has("outbox")) {
      napplet.outbox = {
        getEvent,
        query,
        subscribe: subscribe2,
        publish,
        resolveRelays
      };
    }
    if (domains.has("upload")) {
      napplet.upload = {
        info: info2,
        upload,
        status,
        onStatus
      };
    }
    if (domains.has("intent")) {
      napplet.intent = {
        invoke,
        open: open2,
        available,
        handlers,
        onChanged: onChanged3
      };
    }
    if (domains.has("webrtc")) {
      napplet.webrtc = {
        open: open3,
        send: send2,
        close: close2,
        onEvent: onEvent2
      };
    }
  }
  function installDeviceDomains(domains, napplet) {
    if (domains.has("ble")) {
      napplet.ble = {
        open: open4,
        services,
        read,
        write,
        subscribe: subscribe3,
        unsubscribe,
        close: close3,
        onEvent: onEvent3
      };
    }
    if (domains.has("link")) {
      napplet.link = {
        open: open5
      };
    }
    if (domains.has("count")) {
      napplet.count = {
        query: query2
      };
    }
    if (domains.has("lists")) {
      napplet.lists = {
        supported,
        add,
        remove
      };
    }
    if (domains.has("common")) {
      napplet.common = {
        encodeNip19,
        decodeNip19,
        getProfile: getProfile2,
        follows,
        follow,
        unfollow,
        react,
        report
      };
    }
    if (domains.has("serial")) {
      napplet.serial = {
        open: open6,
        write: write2,
        close: close4,
        onEvent: onEvent4
      };
    }
  }
  function installFilesystemDomains(domains, napplet) {
    if (domains.has("fs")) {
      napplet.fs = {
        info: info3,
        pickFile,
        pickFiles,
        pickDirectory,
        pickSaveFile,
        stat,
        list: list2,
        read: read2,
        write: write3,
        mkdir,
        remove: remove2,
        move,
        watch,
        unwatch,
        onChanged: onChanged4
      };
    }
    if (domains.has("dm")) {
      napplet.dm = {
        status: status2,
        conversations,
        messages,
        send: send3,
        subscribe: subscribe4,
        unsubscribe: unsubscribe2,
        onMessage
      };
    }
  }

  // src/runtime-globals.ts
  function createNappletGlobal(domains) {
    const napplet = {};
    installCoreDomains(domains, napplet);
    installServiceDomains(domains, napplet);
    return napplet;
  }

  // src/runtime.ts
  var DEFAULT_DOMAINS = new Set(NAP_DOMAINS);
  var installedDomainShims = /* @__PURE__ */ new Set();
  var messageListenerInstalled = false;
  var DOMAIN_ROUTERS = [
    ["keys.", handleKeysMessage],
    ["media.", handleMediaMessage],
    ["notify.", handleNotifyMessage],
    ["resource.", handleResourceMessage],
    ["cvm.", handleCvmMessage],
    ["outbox.", handleOutboxMessage],
    ["upload.", handleUploadMessage],
    ["intent.", handleIntentMessage],
    ["inc.", handleIncMessage],
    ["ble.", handleBleMessage],
    ["webrtc.", handleWebrtcMessage],
    ["link.", handleLinkMessage],
    ["count.", handleCountMessage],
    ["lists.", handleListsMessage],
    ["common.", handleCommonMessage],
    ["serial.", handleSerialMessage],
    ["fs.", handleFsMessage],
    ["dm.", handleDmMessage],
    ["identity.", handleIdentityMessage],
    ["theme.", handleThemeMessage],
    ["config.", handleConfigMessage]
  ];
  function handleEnvelopeMessage(event) {
    if (event.source !== window.parent) return;
    const msg = event.data;
    if (typeof msg !== "object" || msg === null || typeof msg.type !== "string") return;
    const typed = msg;
    const type = typed.type;
    for (const [prefix, route] of DOMAIN_ROUTERS) {
      if (type.startsWith(prefix)) {
        route(typed);
        return;
      }
    }
  }
  function normalizeDomains(domains) {
    if (!domains) return new Set(DEFAULT_DOMAINS);
    return new Set(domains.filter((domain) => DEFAULT_DOMAINS.has(domain)));
  }
  function installDomainShim(domain) {
    if (installedDomainShims.has(domain)) return;
    installedDomainShims.add(domain);
    switch (domain) {
      case "relay":
        return;
      case "inc":
        installIncShim();
        return;
      case "storage":
        installStorageShim();
        return;
      case "keys":
        installKeysShim();
        return;
      case "media":
        installMediaShim();
        return;
      case "notify":
        installNotifyShim();
        return;
      case "identity":
        installIdentityShim();
        return;
      case "theme":
        installThemeShim();
        return;
      case "config":
        installConfigShim();
        return;
      case "resource":
        installResourceShim();
        return;
      case "cvm":
        installCvmShim();
        return;
      case "outbox":
        installOutboxShim();
        return;
      case "upload":
        installUploadShim();
        return;
      case "intent":
        installIntentShim();
        return;
      case "ble":
        installBleShim();
        return;
      case "webrtc":
        installWebrtcShim();
        return;
      case "link":
        installLinkShim();
        return;
      case "count":
        installCountShim();
        return;
      case "lists":
        installListsShim();
        return;
      case "serial":
        installSerialShim();
        return;
      case "fs":
        installFsShim();
        return;
      case "common":
        installCommonShim();
        return;
      case "dm":
        installDmShim();
        return;
    }
  }
  function installNappletGlobal(options = {}) {
    const domains = normalizeDomains(options.domains);
    const napplet = createNappletGlobal(domains);
    window.napplet = napplet;
    if (!messageListenerInstalled) {
      window.addEventListener("message", handleEnvelopeMessage);
      messageListenerInstalled = true;
    }
    for (const domain of domains) {
      installDomainShim(domain);
    }
    return napplet;
  }

  // src/prelude.ts
  var KNOWN_DOMAINS = new Set(NAP_DOMAINS);
  function normalizePreludeDomains(options) {
    if (!options || !Array.isArray(options.domains)) {
      throw new TypeError("Napplet runtime prelude requires an explicit domains array");
    }
    return options.domains.filter((domain) => KNOWN_DOMAINS.has(domain));
  }
  function installNappletRuntimePrelude(options) {
    return installNappletGlobal({ domains: normalizePreludeDomains(options) });
  }
  function renderNappletRuntimePreludeCall(options) {
    return `globalThis.NappletShimPrelude.install(${JSON.stringify({
      domains: normalizePreludeDomains(options)
    })});`;
  }
  function renderNappletRuntimePreludeScript(options) {
    return `<script>${renderNappletRuntimePreludeCall(options)}<\/script>`;
  }
  var install = installNappletRuntimePrelude;
  return __toCommonJS(prelude_exports);
})();
//# sourceMappingURL=prelude.global.js.map