import {
  registerVmCallbacks as registerRealVmCallbacks,
  onScoreChanged as onRealScoreChanged,
  onChannelChanged as onRealChannelChanged,
  onChannelDisplayNameChanged as onRealChannelDisplayNameChanged,
  onChannelDisplayNamesChanged as onRealChannelDisplayNamesChanged,
} from './dirplayer-js-api-real.js';

// Stubs for the dirplayer-js-api module.
// In production, these are provided by the Electron host.
export function onMovieLoaded() {}
export function onCastListChanged() {}
export function onCastLibNameChanged() {}
export function onCastMemberListChanged() {}
export function onCastMemberChanged() {}
// BrowserPlayerHandle producer-boundary fixture. The Rust callback receives
// an explicit owner key; route it to the exact registered callback and pass
// the key through so the wasm test can verify ownership and synchronously
// re-enter its handle while the producer's session borrow is released.
const _browserHandleRegistrations = new Map();
const _browserHandleThrowNext = new Set();
function invokeBrowserHandleCallback(ownerKey, callback, kind, payload) {
  if (_browserHandleThrowNext.delete(ownerKey)) {
    throw new Error(`test callback failure for ${ownerKey}`);
  }
  return callback(kind, payload, ownerKey);
}
export function __testRegisterBrowserHandleCallback(ownerKey, callback) {
  _browserHandleRegistrations.get(ownerKey)?.();
  const registration = registerRealVmCallbacks({
    onScoreChanged: (payload) => invokeBrowserHandleCallback(ownerKey, callback, 'score', payload),
    onChannelChanged: (channel, payload) => invokeBrowserHandleCallback(ownerKey, callback, `channel:${channel}`, payload),
    onChannelDisplayNameChanged: (_channel, payload) => invokeBrowserHandleCallback(ownerKey, callback, 'channelName', payload),
    onChannelDisplayNamesChanged: (payload) => invokeBrowserHandleCallback(ownerKey, callback, 'channelNames', payload),
  }, ownerKey);
  _browserHandleRegistrations.set(ownerKey, registration);
}
export function __testThrowNextBrowserHandleCallback(ownerKey) {
  _browserHandleThrowNext.add(ownerKey);
}
export function __testUnregisterBrowserHandleCallback(ownerKey) {
  _browserHandleRegistrations.get(ownerKey)?.();
  _browserHandleRegistrations.delete(ownerKey);
}
export function onScoreChanged(snapshot, ownerKey) {
  onRealScoreChanged(snapshot, ownerKey);
}
export function onChannelChanged(channel, snapshot, ownerKey) {
  onRealChannelChanged(channel, snapshot, ownerKey);
}
export function onChannelDisplayNameChanged(channel, displayName, ownerKey) {
  onRealChannelDisplayNameChanged(channel, displayName, ownerKey);
}
export function onChannelDisplayNamesChanged(names, ownerKey) {
  onRealChannelDisplayNamesChanged(names, ownerKey);
}
export function onFrameChanged() {}
export function onScriptError(data) {
  const msg = data?.message || JSON.stringify(data);
  console.error('[SCRIPT ERROR]', msg, data);
  if (window.__onScriptError) window.__onScriptError(msg);
}
export function onScopeListChanged() {}
export function onBreakpointListChanged() {}
export function onGlobalListChanged() {}
export function onScriptErrorCleared() {}
export function onDebugMessage() {}
export function onDebugContent() {}
export function onMovieLoadFailed() {}

// Timeout handling — mirrors the real Electron host behavior.
// Uses setInterval to call trigger_timeout, which dispatches
// TimeoutTriggered commands through the player's command loop.
const _timeoutHandles = {};
export function onScheduleTimeout(name, periodMs) {
  if (_timeoutHandles[name]) clearInterval(_timeoutHandles[name]);
  _timeoutHandles[name] = setInterval(() => {
    if (window.__wasm_trigger_timeout) window.__wasm_trigger_timeout(name);
  }, periodMs);
}
export function onClearTimeout(name) {
  if (_timeoutHandles[name]) {
    clearInterval(_timeoutHandles[name]);
    delete _timeoutHandles[name];
  }
}
export function onClearTimeouts() {
  for (const name of Object.keys(_timeoutHandles)) {
    clearInterval(_timeoutHandles[name]);
  }
}
export function onClearAllTimeouts() {
  for (const name of Object.keys(_timeoutHandles)) {
    clearInterval(_timeoutHandles[name]);
    delete _timeoutHandles[name];
  }
}

export function onDatumSnapshot() {}
export function onScriptInstanceSnapshot() {}
export function onExternalEvent() {}

// Lazy-load the Flash manager bundle the first time a Flash member
// appears. If the bundle isn't present (e.g. ruffle/ missing) the
// imports resolve to no-ops so tests without Flash still run.
let _flashManager = null;
let _flashManagerPromise = null;
// Existing owner callback routing is the temporary browser-harness boundary.
// The FlashOwnerHost itself remains captured by each registration closure.
const _flashOwnerCallbacks = new Map();

// BrowserTestPlayer registers a non-owning, exact-generation capability here.
// The capability is disposed before the harness retires its player.
export function dirplayer_registerFlashOwner(ownerKey, capability) {
  if (typeof ownerKey !== 'string' || !ownerKey || !capability) return;
  _flashOwnerCallbacks.get(ownerKey)?.dispose();
  let disposed = false;
  let registration;
  const registrationReady = flashManager().then(m => {
    if (disposed) return undefined;
    const ownerRegistration = m.registerFlashOwner?.(ownerKey, capability);
    const bridgeDisposer = m.initFlashBridge?.(capability, ownerRegistration);
    registration = bridgeDisposer
      ? { host: bridgeDisposer.host, dispose: () => bridgeDisposer() }
      : ownerRegistration;
    if (disposed) registration?.dispose?.();
    return registration;
  });
  const callbacks = {
    onLoaded: (...args) => {
      if (disposed) return;
      registrationReady.then(reg => {
        if (!reg || reg.host.disposed) return;
        return flashManager().then(m => m.createFlashInstanceForOwner?.(reg.host, ...args));
      }).catch(e => console.error('createFlashInstance failed:', e));
    },
    onUnloaded: (spriteNum) => {
      if (disposed) return;
      registrationReady.then(reg => {
        if (reg && !reg.host.disposed) flashManager().then(m => m.destroyFlashInstance?.(reg.host, spriteNum));
      });
    },
    onReset: () => {
      if (disposed) return;
      registrationReady.then(reg => {
        if (reg && !reg.host.disposed) flashManager().then(m => m.destroyAllFlashInstances?.(reg.host));
      });
    },
    onPlayOwned: (spriteNum) => {
      if (disposed || !registration) return;
      return flashManager().then(m => m.playFlashForOwner?.(registration.host, spriteNum));
    },
    onLocalConnectionSendOwned: (name, method, argsJson) => {
      if (disposed || !registration || !registration.host || registration.host.disposed) return false;
      return !!_flashManager?.localConnectionSendForOwner?.(registration.host, name, method, argsJson);
    },
    dispose: () => {
      disposed = true;
      registration?.dispose?.();
    },
  };
  _flashOwnerCallbacks.set(ownerKey, callbacks);
  // These routers are stable for the page lifetime and resolve the exact
  // owner closure, so registering a later player cannot replace an earlier
  // owner's play callback. The unqualified legacy LocalConnection function
  // remains the production single-owner path; new owner-aware callers use the
  // explicit suffix below.
  window.dirplayer_rufflePlayOwned = (requestedOwnerKey, spriteNum) =>
    _flashOwnerCallbacks.get(requestedOwnerKey)?.onPlayOwned(spriteNum);
  window.dirplayer_localConnectionSendOwned = (requestedOwnerKey, name, method, argsJson) =>
    _flashOwnerCallbacks.get(requestedOwnerKey)?.onLocalConnectionSendOwned(name, method, argsJson) ?? false;
}
export function dirplayer_unregisterFlashOwner(ownerKey) {
  if (typeof ownerKey !== 'string') return;
  const callbacks = _flashOwnerCallbacks.get(ownerKey);
  _flashOwnerCallbacks.delete(ownerKey);
  callbacks?.dispose();
}
function flashManager() {
  if (_flashManager) return Promise.resolve(_flashManager);
  if (!_flashManagerPromise) {
    _flashManagerPromise = import('./flashPlayerManager.bundle.js')
      .then(mod => {
        // Per-owner registration installs the bridge globals with the actual
        // capability and captures the owner host.  Do not call initFlashBridge
        // without a capability: that would create an unowned bridge or throw
        // before the owner closure exists.
        return (_flashManager = mod);
      })
      .catch(err => {
        console.warn('[flash] bundle not available:', err?.message || err);
        return (_flashManager = { createFlashInstance: () => {}, destroyFlashInstance: () => {} });
      });
  }
  return _flashManagerPromise;
}

// Install the bridge globals EAGERLY rather than on the first Flash member.
// The dev app does this from callbacks.ts at startup; deferring it until the
// lazy bundle import resolves is too late, because a movie can call a Flash
// sprite method during the init sequence. monsterattack's `on streamStatus`
// (dispatched from run_movie_init_sequence) does `sprite(2).gotoFrame(pct)`,
// and the wasm extern then hit an undefined `dirplayer_ruffleGoToFrame` —
// an uncaught ReferenceError that aborted init, so no sprite ever entered
// and the playhead never left frame 1. Kicking the import off here closes
// that window. Bundle-missing still degrades to no-ops via the catch above.
flashManager();

export function onFlashMemberLoaded(spriteNum, castLib, castMember, swfData, width, height, pausedAtStart, assertedFrame, ownerKey) {
  const copy = new Uint8Array(swfData);
  _flashOwnerCallbacks.get(ownerKey)?.onLoaded(spriteNum, castLib, castMember, copy, width, height, pausedAtStart, assertedFrame);
}
export function onFlashMemberUnloaded(spriteNum, ownerKey) {
  _flashOwnerCallbacks.get(ownerKey)?.onUnloaded(spriteNum);
}
export function onFlashResetAll(ownerKey) {
  // Only tear down if the Flash bundle was actually loaded by a prior movie;
  // don't import it just to reset nothing on a pure non-Flash test run.
  if (typeof ownerKey !== 'string' || ownerKey.length === 0) return;
  _flashOwnerCallbacks.get(ownerKey)?.onReset();
}
export function onStageSizeChanged() {}

// External Xtra plugin bridge — delegated to the real implementation.
//
// `dirplayer-js-api-real.js` is a copy of `dirplayer-js-api/index.js`
// dropped here by `run-browser-tests.mjs`. We re-export its xtra-
// loading functions so e2e tests can actually exercise the SDK end to
// end (`new(xtra "BobbaXtra")`, registry resolution, on-demand loads).
// The HTML template calls `setVmModule(wasm)` after init so the real
// bridge can route plugin host calls back into the test wasm.
//
// Note: we forward only the xtra-bridge surface here, not the UI
// callbacks (`onMovieLoaded`, etc.). The real index.js's UI callbacks
// delegate to `vmCallbacks` which the test harness never registers —
// so they'd throw NPEs. Test mode keeps its own no-op UI stubs above.
export {
  dispatchExternalXtraStaticHandler,
  dispatchExternalXtraInstanceHandler,
  createExternalXtraInstance,
  destroyExternalXtraInstance,
  externalXtraHasStaticHandler,
  loadExternalXtra,
  loadExternalXtras,
  getExternalXtrasReady,
  disposeExternalXtraHost,
  onRequestXtraLoad,
  setXtraRegistry,
  getXtraRegistry,
  setXtraMovieBase,
  setXtraHostBase,
  setVmModule,
  loadDefaultXtraRegistry,
  resolveAndLoadMovieXtras,
} from './dirplayer-js-api-real.js';
