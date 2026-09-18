import {
  registerVmCallbacks as registerRealVmCallbacks,
  dispatchVmCallback as dispatchRealVmCallback,
  onFlashMemberLoaded as onRealFlashMemberLoaded,
  onFlashMemberLoadedPrepared as onRealFlashMemberLoadedPrepared,
  onFlashMemberResized as onRealFlashMemberResized,
  onFlashMemberUnloaded as onRealFlashMemberUnloaded,
  onFlashMemberUnloadedAtGeneration as onRealFlashMemberUnloadedAtGeneration,
  onFlashResetAll as onRealFlashResetAll,
  onScoreChanged as onRealScoreChanged,
  onChannelChanged as onRealChannelChanged,
  onChannelDisplayNameChanged as onRealChannelDisplayNameChanged,
  onChannelDisplayNamesChanged as onRealChannelDisplayNamesChanged,
  onDebugMessageOwned as onRealDebugMessageOwned,
  onScriptErrorOwned as onRealScriptErrorOwned,
  onDatumSnapshotOwned as onRealDatumSnapshotOwned,
  onScriptInstanceSnapshotOwned as onRealScriptInstanceSnapshotOwned,
  dirplayer_ruffleGetVariableOwnedForBinding as onRealRuffleGetVariableOwnedForBinding,
  dirplayer_ruffleGetVariableOwnedAtGeneration as onRealRuffleGetVariableOwnedAtGeneration,
  dirplayer_ruffleGetSpriteVariableOwnedAtGeneration as onRealRuffleGetSpriteVariableOwnedAtGeneration,
  dirplayer_isFlashInstanceReadyOwned as onRealFlashInstanceReadyOwned,
  dirplayer_ruffleSetVariableOwnedAtGeneration as onRealRuffleSetVariableOwnedAtGeneration,
  dirplayer_ruffleCallFunctionOwnedAtGeneration as onRealRuffleCallFunctionOwnedAtGeneration,
} from './dirplayer-js-api-real.js';

const _realVmOwnerKeys = new Set();
let _failNextNestedCallbackRegistration = false;
let _lastFailedNestedOwnerKey = null;
function registerTrackedVmCallbacks(callbacks, ownerKey, setAsDefault = true) {
  const dispose = registerRealVmCallbacks(callbacks, ownerKey, setAsDefault);
  if (ownerKey) _realVmOwnerKeys.add(ownerKey);
  return () => {
    dispose?.();
    if (ownerKey) _realVmOwnerKeys.delete(ownerKey);
  };
}

const _browserHandleTimers = new Map();
function scheduleBrowserHandleTimer(ownerKey, handle, name, period, incarnation) {
  if (!Number.isFinite(period) || period <= 0) return;
  const timers = _browserHandleTimers.get(ownerKey) || new Map();
  const previous = timers.get(name);
  if (previous && previous.incarnation !== incarnation) clearInterval(previous.handle);
  const reservation = { incarnation, handle: undefined };
  timers.set(name, reservation);
  _browserHandleTimers.set(ownerKey, timers);
  const intervalHandle = setInterval(() => {
    if (_browserHandleTimers.get(ownerKey)?.get(name) !== reservation) return;
    try {
      handle.trigger_timeout(name, incarnation);
    } catch {
      clearBrowserHandleTimer(ownerKey, name, incarnation);
    }
  }, period);
  if (timers.get(name) === reservation) reservation.handle = intervalHandle;
  else clearInterval(intervalHandle);
}
function clearBrowserHandleTimer(ownerKey, name, incarnation) {
  const timers = _browserHandleTimers.get(ownerKey);
  const current = timers?.get(name);
  if (!current || current.incarnation !== incarnation) return;
  clearInterval(current.handle);
  timers.delete(name);
}

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
function invokeBrowserHandleCallback(ownerKey, callback, kind, rebindHandle, rebindOnChannel, ...payload) {
  if (_browserHandleThrowNext.delete(ownerKey)) {
    throw new Error(`test callback failure for ${ownerKey}`);
  }
  const result = callback(kind, ...payload, ownerKey);
  if (rebindOnChannel && rebindHandle && kind === 'channel:7') {
    rebindHandle.set_debug_selected_channel(8);
    rebindHandle.set_host_event_sink((event, owner) =>
      callback(`host:${String(event.type)}`, event, owner));
  }
  return result;
}
export function __testRegisterBrowserHandleCallback(ownerKey, callback, rebindHandle, rebindOnChannel = false) {
  _browserHandleRegistrations.get(ownerKey)?.();
  const registration = registerRealVmCallbacks({
    onScoreChanged: (payload) => invokeBrowserHandleCallback(ownerKey, callback, 'score', rebindHandle, rebindOnChannel, payload),
    onChannelChanged: (channel, payload) => invokeBrowserHandleCallback(ownerKey, callback, `channel:${channel}`, rebindHandle, rebindOnChannel, payload),
    onChannelDisplayNameChanged: (_channel, payload) => invokeBrowserHandleCallback(ownerKey, callback, 'channelName', rebindHandle, rebindOnChannel, payload),
    onChannelDisplayNamesChanged: (payload) => invokeBrowserHandleCallback(ownerKey, callback, 'channelNames', rebindHandle, rebindOnChannel, payload),
    onDebugMessage: (message) => invokeBrowserHandleCallback(ownerKey, callback, 'debug', rebindHandle, rebindOnChannel, message),
    onScriptError: (data) => invokeBrowserHandleCallback(ownerKey, callback, 'scriptError', rebindHandle, rebindOnChannel, data),
    onDatumSnapshot: (datumRef, snapshot) => invokeBrowserHandleCallback(ownerKey, callback, 'datum', rebindHandle, rebindOnChannel, datumRef, snapshot),
    onScriptInstanceSnapshot: (instanceId, snapshot) => invokeBrowserHandleCallback(ownerKey, callback, 'scriptInstance', rebindHandle, rebindOnChannel, instanceId, snapshot),
    onScheduleTimeoutOwned: (name, period, incarnation) => scheduleBrowserHandleTimer(ownerKey, rebindHandle, name, period, incarnation),
    onClearTimeoutOwned: (name, incarnation) => clearBrowserHandleTimer(ownerKey, name, incarnation),
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
export function onDebugMessageOwned(ownerKey, message) {
  return onRealDebugMessageOwned(ownerKey, message);
}
export function onScriptErrorOwned(ownerKey, data) {
  return onRealScriptErrorOwned(ownerKey, data);
}
export function onDatumSnapshotOwned(ownerKey, datumRef, snapshot) {
  return onRealDatumSnapshotOwned(ownerKey, datumRef, snapshot);
}
export function onScriptInstanceSnapshotOwned(ownerKey, instanceId, snapshot) {
  return onRealScriptInstanceSnapshotOwned(ownerKey, instanceId, snapshot);
}
// Forward the owner/generation Flash routes exactly as production does. The
// real module returns an explicit unknown-owner envelope when the route is
// absent; this fixture must never fall back to the legacy global bridge.
export function dirplayer_ruffleGetVariableOwnedForBinding(ownerKey, spriteNum, path, returnAsObject = false) {
  return onRealRuffleGetVariableOwnedForBinding(ownerKey, spriteNum, path, returnAsObject);
}
export function dirplayer_ruffleGetVariableOwnedAtGeneration(ownerKey, spriteNum, generation, path, returnAsObject = false) {
  return onRealRuffleGetVariableOwnedAtGeneration(ownerKey, spriteNum, generation, path, returnAsObject);
}
export function dirplayer_ruffleGetSpriteVariableOwnedAtGeneration(ownerKey, spriteNum, generation, path) {
  return onRealRuffleGetSpriteVariableOwnedAtGeneration(ownerKey, spriteNum, generation, path);
}
export function dirplayer_isFlashInstanceReadyOwned(ownerKey, spriteNum, generation) {
  return onRealFlashInstanceReadyOwned(ownerKey, spriteNum, generation);
}
export function dirplayer_ruffleSetVariableOwnedAtGeneration(ownerKey, spriteNum, generation, path, value) {
  return onRealRuffleSetVariableOwnedAtGeneration(ownerKey, spriteNum, generation, path, value);
}
export function dirplayer_ruffleCallFunctionOwnedAtGeneration(ownerKey, spriteNum, generation, path, argsXml) {
  return onRealRuffleCallFunctionOwnedAtGeneration(ownerKey, spriteNum, generation, path, argsXml);
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

export function onScheduleTimeoutOwned(ownerKey, name, periodMs, incarnation) {
  dispatchRealVmCallback(ownerKey, 'onScheduleTimeoutOwned', name, periodMs, incarnation, ownerKey);
}
export function onClearTimeoutOwned(ownerKey, name, incarnation) {
  dispatchRealVmCallback(ownerKey, 'onClearTimeoutOwned', name, incarnation, ownerKey);
}

export function onDatumSnapshot() {}
export function onScriptInstanceSnapshot() {}
export function onExternalEvent() {}

// Lazy-load the Flash manager bundle the first time a Flash member
// appears. If the bundle isn't present (e.g. ruffle/ missing) the
// imports resolve to no-ops so tests without Flash still run.
let _flashManager = null;
let _flashManagerPromise = null;
let _nestedBrowserOwnerController = null;
const _nestedBrowserOwnerKeys = new Set();
// Existing owner callback routing is the temporary browser-harness boundary.
// The BrowserOwnerHost itself remains captured by each registration closure.
const _browserOwnerCallbacks = new Map();
const _browserOwnerTimerResetReentry = new Map();
const _browserOwnerTimerClearCounts = new Map();
const _browserOwnerTimerClearPhases = new Map();
const _browserOwnerTimerClearPhase = new Map();
const browserOwnerTimerClearKey = (ownerKey, name, incarnation) => `${ownerKey}\u0000${name}\u0000${incarnation}`;
function installProductionNestedBrowserController(manager) {
  if (typeof window.dirplayer_registerNestedBrowserOwner === 'function') return;
  if (typeof manager.createBrowserOwnerController !== 'function') {
    throw new Error('production nested browser owner controller factory is unavailable');
  }
  _nestedBrowserOwnerController = manager.createBrowserOwnerController((callbacks, ownerKey, setAsDefault) =>
    (() => {
      if (_failNextNestedCallbackRegistration) {
        _failNextNestedCallbackRegistration = false;
        _lastFailedNestedOwnerKey = ownerKey;
        onRealFlashMemberLoadedPrepared(
          1,
          1,
          1,
          new Uint8Array([70, 87, 83]),
          1,
          1,
          true,
          1,
          ownerKey,
          1,
        );
        return registerRealVmCallbacks({
          ...callbacks,
          onFlashMemberLoaded: () => {
            throw new Error(`test nested callback flush failure for ${ownerKey}`);
          },
        }, ownerKey, setAsDefault);
      }
      return registerTrackedVmCallbacks(callbacks, ownerKey, setAsDefault);
    })());
  window.dirplayer_registerNestedBrowserOwner = (parentOwnerKey, childOwnerKey, capability) => {
    const result = _nestedBrowserOwnerController.registerNested(parentOwnerKey, childOwnerKey, capability);
    _nestedBrowserOwnerKeys.add(childOwnerKey);
    return result;
  };
  window.dirplayer_retireNestedBrowserOwner = (parentOwnerKey, childOwnerKey) => {
    try {
      return _nestedBrowserOwnerController.retireNested(parentOwnerKey, childOwnerKey);
    } finally {
      _nestedBrowserOwnerKeys.delete(childOwnerKey);
    }
  };
}

export function dirplayer_testNestedBrowserOwnerKeys() {
  return Array.from(_nestedBrowserOwnerKeys);
}

export function dirplayer_testDispatchBrowserOwnerAction(ownerKey, method, ...args) {
  if (!_realVmOwnerKeys.has(ownerKey)) return false;
  dispatchRealVmCallback(ownerKey, method, ...args);
  return true;
}

export function dirplayer_testQueuePreparedFlashAction(ownerKey) {
  onRealFlashMemberLoadedPrepared(
    1,
    1,
    1,
    new Uint8Array([70, 87, 83]),
    1,
    1,
    true,
    1,
    ownerKey,
    1,
  );
}

export function dirplayer_testProbePreparedBrowserOwner(ownerKey) {
  let adopted = false;
  const dispose = registerRealVmCallbacks({
    onFlashMemberLoaded: () => { adopted = true; },
  }, ownerKey, false);
  dispose?.();
  return adopted;
}

export function dirplayer_testFailNextNestedCallbackRegistration() {
  _failNextNestedCallbackRegistration = true;
}

export function dirplayer_testLastFailedNestedOwnerKey() {
  return _lastFailedNestedOwnerKey;
}

// The failed registration deliberately installs a callback that throws during
// the real registrar's synchronous prepared-action flush. If rollback leaked
// the callback table, this direct production dispatch would still throw.
export function dirplayer_testProbeFailedNestedCallbackAbsent(ownerKey) {
  try {
    dispatchRealVmCallback(ownerKey, 'onFlashMemberLoaded');
    return true;
  } catch {
    return false;
  }
}

// Probe the manager's owner route rather than the fixture's bookkeeping set.
// A disposed child host must be rejected by the production generation-aware
// route with the typed unknown-owner result.
export function dirplayer_testProbeFailedNestedHostAbsent(ownerKey) {
  try {
    const result = onRealRuffleGetVariableOwnedForBinding(ownerKey, 1, '/', false);
    return result?.ok === false && result.code === 'unknown-owner';
  } catch {
    return false;
  }
}

// BrowserTestPlayer registers a non-owning, exact-generation capability here.
// The capability is disposed before the harness retires its player.
export function dirplayer_registerBrowserOwner(ownerKey, capability) {
  if (typeof ownerKey !== 'string' || !ownerKey || !capability) return;
  _browserOwnerCallbacks.get(ownerKey)?.dispose();
  let disposed = false;
  let registration;
  const registrationReady = flashManager().then(m => {
    if (disposed) return undefined;
    installProductionNestedBrowserController(m);
    const ownerRegistration = m.registerBrowserOwner?.(ownerKey, capability);
    const bridgeDisposer = m.initFlashBridge?.(capability, ownerRegistration);
    if (!bridgeDisposer && !ownerRegistration) {
      throw new Error(`browser owner ${ownerKey} could not be registered`);
    }
    const host = bridgeDisposer?.host ?? ownerRegistration?.host;
    if (!host || typeof m.createOwnedBrowserCallbacks !== 'function') {
      bridgeDisposer?.();
      if (!bridgeDisposer) ownerRegistration?.dispose?.();
      throw new Error(`browser owner ${ownerKey} lacks the production callback factory`);
    }
    const ownedCallbacks = m.createOwnedBrowserCallbacks(host);
    const reentryCallbacks = {
      ...ownedCallbacks,
      onScheduleTimeoutOwned: (name, periodMs, incarnation, callbackOwnerKey) => {
        const result = ownedCallbacks.onScheduleTimeoutOwned(name, periodMs, incarnation, callbackOwnerKey);
        const handle = _browserOwnerTimerResetReentry.get(ownerKey);
        if (handle) {
          _browserOwnerTimerResetReentry.delete(ownerKey);
          _browserOwnerTimerClearPhase.set(ownerKey, 'during-reset');
          try {
            handle.reset();
          } finally {
            _browserOwnerTimerClearPhase.set(ownerKey, 'post-reset');
          }
        }
        return result;
      },
      onClearTimeoutOwned: (name, incarnation, callbackOwnerKey) => {
        const key = browserOwnerTimerClearKey(callbackOwnerKey, name, incarnation);
        _browserOwnerTimerClearCounts.set(key, (_browserOwnerTimerClearCounts.get(key) || 0) + 1);
        const phases = _browserOwnerTimerClearPhases.get(key) || [];
        phases.push(_browserOwnerTimerClearPhase.get(callbackOwnerKey) || 'outside-reentry');
        _browserOwnerTimerClearPhases.set(key, phases);
        return ownedCallbacks.onClearTimeoutOwned(name, incarnation, callbackOwnerKey);
      },
    };
    Object.assign(callbacks, reentryCallbacks);
    const disposeCallbacks = registerTrackedVmCallbacks(reentryCallbacks, ownerKey, false);
    registration = {
      host,
      dispose: () => {
        disposeCallbacks?.();
        bridgeDisposer?.();
        if (!bridgeDisposer) ownerRegistration?.dispose?.();
      },
    };
    if (disposed) registration?.dispose?.();
    return registration;
  });
  const callbacks = {
    dispose: () => {
      disposed = true;
      registration?.dispose?.();
    },
  };
  _browserOwnerCallbacks.set(ownerKey, callbacks);
  registrationReady.catch(error => {
    callbacks.dispose();
    console.error(`browser owner ${ownerKey} registration failed:`, error);
  });
  // These routers are stable for the page lifetime and resolve the exact
  // owner closure, so registering a later player cannot replace an earlier
  // owner's play callback. The unqualified legacy LocalConnection function
  // remains the production single-owner path; new owner-aware callers use the
  // explicit suffix below.
  window.dirplayer_rufflePlayOwned = (requestedOwnerKey, spriteNum) =>
    dispatchRealVmCallback(requestedOwnerKey, 'onFlashPlayOwned', spriteNum);
  window.dirplayer_localConnectionSendOwned = (requestedOwnerKey, name, method, argsJson) =>
    dispatchRealVmCallback(requestedOwnerKey, 'onFlashLocalConnectionSendOwned', name, method, argsJson) ?? false;
  return registrationReady;
}
export function dirplayer_unregisterBrowserOwner(ownerKey) {
  if (typeof ownerKey !== 'string') return;
  const callbacks = _browserOwnerCallbacks.get(ownerKey);
  _browserOwnerCallbacks.delete(ownerKey);
  callbacks?.dispose();
}

export function dirplayer_testInstallBrowserOwnerTimerResetReentry(ownerKey, handle) {
  if (typeof ownerKey !== 'string' || !handle || typeof handle.reset !== 'function') return false;
  _browserOwnerTimerResetReentry.set(ownerKey, handle);
  return true;
}

export function dirplayer_testClearBrowserOwnerTimerResetReentry(ownerKey) {
  _browserOwnerTimerResetReentry.delete(ownerKey);
}

export function dirplayer_testResetBrowserOwnerTimerClearRecord(ownerKey, name, incarnation) {
  const key = browserOwnerTimerClearKey(ownerKey, name, incarnation);
  _browserOwnerTimerClearCounts.delete(key);
  _browserOwnerTimerClearPhases.delete(key);
  _browserOwnerTimerClearPhase.delete(ownerKey);
}

export function dirplayer_testBrowserOwnerTimerClearCount(ownerKey, name, incarnation) {
  return _browserOwnerTimerClearCounts.get(browserOwnerTimerClearKey(ownerKey, name, incarnation)) || 0;
}

export function dirplayer_testBrowserOwnerTimerClearPhases(ownerKey, name, incarnation) {
  return _browserOwnerTimerClearPhases.get(browserOwnerTimerClearKey(ownerKey, name, incarnation)) || [];
}

export async function dirplayer_testBrowserOwnerTimerProbe(
  ownerKey,
  name,
  period,
  incarnation,
  operation = 'schedule',
) {
  const callbacks = _browserOwnerCallbacks.get(ownerKey);
  const canProbeCurrent = operation === 'current' && _realVmOwnerKeys.has(ownerKey);
  if ((!callbacks && !canProbeCurrent) || typeof name !== 'string' || !Number.isSafeInteger(incarnation) || incarnation <= 0) return false;
  if (operation === 'schedule') {
    dispatchRealVmCallback(ownerKey, 'onScheduleTimeoutOwned', name, period, incarnation, ownerKey);
  } else if (operation === 'clear') {
    dispatchRealVmCallback(ownerKey, 'onClearTimeoutOwned', name, incarnation, ownerKey);
  }
  const manager = await flashManager();
  return operation === 'current'
    ? manager.isBrowserOwnerTimerCurrent?.(ownerKey, name, incarnation) === true
    : true;
}

// Owner-bound Lingo callback exports mirror the production browser module.
// The template only forwards the exact window route and accepts a strict
// boolean acknowledgement; it never falls back to a current owner.
export function registerLingoCallbackOwned(ownerKey, ...args) {
  const register = globalThis.window?.dirplayer_registerLingoCallbackOwned;
  return typeof register === 'function' ? register(ownerKey, ...args) === true : false;
}
export function dirplayer_registerLingoCallbackOwned(ownerKey, ...args) {
  return registerLingoCallbackOwned(ownerKey, ...args);
}
export function triggerLingoCallbackOnScriptRuffle(...args) {
  const trigger = globalThis.window?.dirplayer_triggerLingoCallbackOnScriptRuffle
    ?? globalThis.window?.dirplayer_triggerLingoCallbackOnScript;
  return typeof trigger === 'function' ? trigger(...args) === true : false;
}

// BrowserPlayerHandle capability regression helper. This instantiates the
// production BrowserOwnerHost with the actual wasm capability object supplied by
// the Rust harness; it does not substitute a mock setter or expose host state.
export async function dirplayer_testBrowserOwnerCapability(ownerKey, capability, observe) {
  const manager = await flashManager();
  if (typeof manager.BrowserOwnerHost !== 'function') {
    throw new Error('BrowserOwnerHost production class is unavailable');
  }
  const host = new manager.BrowserOwnerHost(ownerKey, capability);
  const ticket = host.beginScriptedAccess(1);
  if (ticket === undefined) throw new Error('BrowserOwnerHost rejected live capability');
  observe?.('begin');
  host.completeScriptedAccess(1, ticket);
  observe?.('complete');
  if (host.scriptedAccessRequestCount() !== 0) {
    throw new Error('BrowserOwnerHost retained completed scripted access');
  }
  const secondTicket = host.beginScriptedAccess(1);
  if (secondTicket === undefined) throw new Error('BrowserOwnerHost rejected second live capability wait');
  observe?.('begin-again');
  host.dispose();
  observe?.('dispose');
  return true;
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
  return onRealFlashMemberLoaded(spriteNum, castLib, castMember, swfData, width, height, pausedAtStart, assertedFrame, ownerKey);
}
export function onFlashMemberLoadedPrepared(spriteNum, castLib, castMember, swfData, width, height, pausedAtStart, assertedFrame, ownerKey, generation) {
  return onRealFlashMemberLoadedPrepared(spriteNum, castLib, castMember, swfData, width, height, pausedAtStart, assertedFrame, ownerKey, generation);
}
export function onFlashMemberResized(spriteNum, generation, width, height, ownerKey) {
  return onRealFlashMemberResized(spriteNum, generation, width, height, ownerKey);
}
export function onFlashMemberUnloaded(spriteNum, ownerKey) {
  return onRealFlashMemberUnloaded(spriteNum, ownerKey);
}
export function onFlashMemberUnloadedAtGeneration(spriteNum, generation, ownerKey) {
  return onRealFlashMemberUnloadedAtGeneration(spriteNum, generation, ownerKey);
}
export function onFlashResetAll(ownerKey) {
  return onRealFlashResetAll(ownerKey);
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

// The generated wasm module imports these owner-routed Flash ABI names
// directly. Forward them through the real production bridge so the browser
// fixture exercises the same window controller without duplicating it here.
export {
  registerNestedBrowserOwner,
  retireNestedBrowserOwner,
} from './dirplayer-js-api-real.js';
