let vmCallbacks = undefined;
const vmCallbacksByOwner = new Map();

// Owner-qualified Flash calls use the same capability-scoped callback table as
// the rest of the VM bridge.  These stable page functions deliberately do not
// close over a particular player: registration and disposal update the
// owner-keyed entry, so a later owner cannot replace or resurrect an earlier
// runtime and non-LIFO teardown leaves no stale closure behind.
const routeFlashPlayOwned = (ownerKey, spriteNum) =>
  vmCallbacksByOwner.get(ownerKey)?.onFlashPlayOwned?.(spriteNum);
const routeFlashLocalConnectionSendOwned = (ownerKey, name, method, argsJson) =>
  vmCallbacksByOwner.get(ownerKey)?.onFlashLocalConnectionSendOwned?.(name, method, argsJson) ?? false;

// Generation-aware Flash routes are installed by FlashPlayerManager. These
// wrappers deliberately call only the exact owner route and return a typed
// failure when the route is absent; they never fall back to legacy globals or
// the current VM callback provider.
const invokeOwnedFlashGenerationRoute = (name, args) => {
  const route = globalThis.window?.[name];
  if (typeof route !== 'function') return { ok: false, code: 'unknown-owner' };
  return route(...args);
};

export function dirplayer_ruffleGetVariableOwnedForBinding(ownerKey, spriteNum, path, returnAsObject = false) {
  return invokeOwnedFlashGenerationRoute('dirplayer_ruffleGetVariableOwnedForBinding', [ownerKey, spriteNum, path, returnAsObject]);
}

export function dirplayer_ruffleGetVariableOwnedAtGeneration(ownerKey, spriteNum, generation, path, returnAsObject = false) {
  return invokeOwnedFlashGenerationRoute('dirplayer_ruffleGetVariableOwnedAtGeneration', [ownerKey, spriteNum, generation, path, returnAsObject]);
}

export function dirplayer_isFlashInstanceReadyOwned(ownerKey, spriteNum, generation) {
  return invokeOwnedFlashGenerationRoute('dirplayer_isFlashInstanceReadyOwned', [ownerKey, spriteNum, generation]);
}

export function dirplayer_ruffleSetVariableOwnedAtGeneration(ownerKey, spriteNum, generation, path, value) {
  return invokeOwnedFlashGenerationRoute('dirplayer_ruffleSetVariableOwnedAtGeneration', [ownerKey, spriteNum, generation, path, value]);
}

export function dirplayer_ruffleCallFunctionOwnedAtGeneration(ownerKey, spriteNum, generation, path, argsXml) {
  return invokeOwnedFlashGenerationRoute('dirplayer_ruffleCallFunctionOwnedAtGeneration', [ownerKey, spriteNum, generation, path, argsXml]);
}

if (typeof globalThis.window !== 'undefined') {
  const win = globalThis.window;
  if (typeof win.dirplayer_rufflePlayOwned !== 'function') {
    win.dirplayer_rufflePlayOwned = routeFlashPlayOwned;
  }
  if (typeof win.dirplayer_localConnectionSendOwned !== 'function') {
    win.dirplayer_localConnectionSendOwned = routeFlashLocalConnectionSendOwned;
  }
}

/** Dispatch an owner-bound VM callback without consulting a current player. */
export function dispatchVmCallback(ownerKey, name, ...args) {
  const callbacks = vmCallbacksByOwner.get(ownerKey);
  const callback = callbacks?.[name];
  if (typeof callback !== 'function') return undefined;
  return callback(...args);
}

// These routes are used by owner-bound Rust callbacks.  They intentionally
// never fall back to vmCallbacks: an unknown or retired owner must be dropped
// rather than delivered to whichever provider registered most recently.
export function onDebugMessageOwned(ownerKey, message) {
  return dispatchVmCallback(ownerKey, 'onDebugMessage', message);
}

export function onScriptErrorOwned(ownerKey, data) {
  return dispatchVmCallback(ownerKey, 'onScriptError', data);
}

export function onDatumSnapshotOwned(ownerKey, datumRef, snapshot) {
  return dispatchVmCallback(ownerKey, 'onDatumSnapshot', datumRef, snapshot);
}

export function onScriptInstanceSnapshotOwned(ownerKey, instanceId, snapshot) {
  return dispatchVmCallback(ownerKey, 'onScriptInstanceSnapshot', instanceId, snapshot);
}

export function registerVmCallbacks(callbacks, ownerKey) {
  vmCallbacks = callbacks;
  if (ownerKey) vmCallbacksByOwner.set(ownerKey, callbacks);
  // Registration is capability-scoped: a provider may dispose its callbacks
  // without clearing a newer provider's registration.
  return () => {
    if (vmCallbacks === callbacks) vmCallbacks = undefined;
    if (ownerKey && vmCallbacksByOwner.get(ownerKey) === callbacks) {
      vmCallbacksByOwner.delete(ownerKey);
    }
  };
}

// Legacy no-owner waiters remain separate from handle-owned waiters.
const _movieLoadedResolvers = [];

/**
 * Create a callback sink whose waiter state belongs to one BrowserPlayerHandle.
 * The returned function is passed directly to Rust; no owner-keyed module map
 * is consulted for owner-qualified events.
 */
export function createVmHostEventSink(onEvent) {
  const waiters = new Set();
  let disposed = false;
  let activeOwnerKey;
  const rejectWaiters = (reason, ownerKey) => {
    const pending = [...waiters].filter((waiter) => ownerKey === undefined || waiter.ownerKey === ownerKey);
    for (const waiter of pending) waiters.delete(waiter);
    for (const waiter of pending) {
      try { waiter.reject(new Error(reason)); } catch (e) { console.error('movie waiter rejection threw:', e); }
    }
  };
  const sink = (event, ownerKey) => {
    if (disposed) return;
    const effectiveOwnerKey = event?.ownerKey ?? ownerKey;
    const ownedEvent = { ...event, ownerKey: effectiveOwnerKey };
    if (event?.type === 'ownerBound') {
      activeOwnerKey = effectiveOwnerKey;
    } else if (event?.type === 'ownerRetired') {
      rejectWaiters('VM host event owner retired', effectiveOwnerKey);
      if (activeOwnerKey === effectiveOwnerKey) activeOwnerKey = undefined;
    } else if (event?.type === 'flashReset') {
      rejectWaiters('VM host event sink owner reset', ownerKey);
      if (activeOwnerKey === ownerKey) activeOwnerKey = undefined;
    }
    if (event?.type === 'movieLoaded' && activeOwnerKey === ownerKey) {
      const pending = [...waiters].filter((waiter) => waiter.ownerKey === ownerKey);
      for (const waiter of pending) waiters.delete(waiter);
      for (const waiter of pending) {
        try { waiter.resolve(event); } catch (e) { console.error('movie waiter resolver threw:', e); }
      }
    } else if (event?.type === 'movieLoadFailed' && activeOwnerKey === ownerKey) {
      rejectWaiters(event.error, ownerKey);
    }
    onEvent(ownedEvent);
  };
  return {
    sink,
    whenMovieLoaded(ownerKey = activeOwnerKey) {
      if (disposed) return Promise.reject(new Error('VM host event sink disposed'));
      if (ownerKey === undefined) return Promise.reject(new Error('VM host event sink has no active owner'));
      return new Promise((resolve, reject) => waiters.add({ resolve, reject, ownerKey }));
    },
    cancelMovieLoadedWaiters(reason = 'movie owner reset', ownerKey) {
      rejectWaiters(reason, ownerKey);
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      rejectWaiters('VM host event sink disposed');
      activeOwnerKey = undefined;
    },
  };
}

/// Returns a Promise that resolves the NEXT time onMovieLoaded fires
/// from vm-rust. Use this between `load_movie_file(path, false)` and
/// `resolveAndLoadMovieXtras()` / `play()` to ensure the movie's
/// metadata (incl. XTRl) has been parsed.
export function whenMovieLoaded() {
  return new Promise((resolve) => _movieLoadedResolvers.push(resolve));
}

export function onMovieLoaded(result) {
  if (vmCallbacks?.onMovieLoaded) {
    vmCallbacks.onMovieLoaded(result);
  }
  // Drain pending whenMovieLoaded() promises.
  const resolvers = _movieLoadedResolvers.splice(0);
  for (const r of resolvers) {
    try { r(result); } catch (e) { console.error('whenMovieLoaded resolver threw:', e); }
  }
}

export function onMovieLoadFailed(path, error) {
  if (vmCallbacks?.onMovieLoadFailed) {
    vmCallbacks.onMovieLoadFailed(path, error);
  } else {
    console.error('[dirplayer] Movie load failed:', path, error);
  }
}

export function onCastListChanged(castList) {
  vmCallbacks.onCastListChanged(castList)
}

export function onCastLibNameChanged(castId, name) {
  vmCallbacks.onCastLibNameChanged(castId, name)
}

export function onCastMemberListChanged(castNumber, members) {
  vmCallbacks.onCastMemberListChanged(castNumber, members)
}

export function onCastMemberChanged(...args) {
  vmCallbacks.onCastMemberChanged(...args)
}

export function onScoreChanged(snapshot, ownerKey) {
  const callbacks = ownerKey ? vmCallbacksByOwner.get(ownerKey) : vmCallbacks;
  if (ownerKey && !callbacks) return;
  callbacks?.onScoreChanged?.(snapshot);
}

export function onFrameChanged(frame) {
  vmCallbacks.onFrameChanged(frame)
}

export function onScriptError(err) {
  vmCallbacks.onScriptError(err)
}

export function onScopeListChanged(scopeList) {
  vmCallbacks.onScopeListChanged(scopeList)
}

export function onBreakpointListChanged(breakpointList) {
  vmCallbacks.onBreakpointListChanged(breakpointList)
}

export function onScriptErrorCleared() {
  vmCallbacks.onScriptErrorCleared()
}

export function onGlobalListChanged(globalList) {
  vmCallbacks.onGlobalListChanged(globalList)
}

export function onDebugMessage(message) {
  vmCallbacks.onDebugMessage(message)
}

export function onDebugContent(content) {
  vmCallbacks.onDebugContent(content)
}

export function onScheduleTimeout(name, period) {
  vmCallbacks.onScheduleTimeout(name, period)
}

export function onClearTimeout(name) {
  vmCallbacks.onClearTimeout(name)
}

export function onClearAllTimeouts() {
  vmCallbacks.onClearAllTimeouts()
}

export function onDatumSnapshot(datumRef, snapshot) {
  vmCallbacks.onDatumSnapshot(datumRef, snapshot)
}

export function onScriptInstanceSnapshot(instanceId, snapshot) {
  vmCallbacks.onScriptInstanceSnapshot(instanceId, snapshot)
}

export function onChannelChanged(channel, value, ownerKey) {
  const callbacks = ownerKey ? vmCallbacksByOwner.get(ownerKey) : vmCallbacks;
  if (ownerKey && !callbacks) return;
  callbacks?.onChannelChanged?.(channel, value);
}

export function onChannelDisplayNameChanged(channel, displayName, ownerKey) {
  const callbacks = ownerKey ? vmCallbacksByOwner.get(ownerKey) : vmCallbacks;
  if (ownerKey && !callbacks) return;
  callbacks?.onChannelDisplayNameChanged?.(channel, displayName);
}

// Bulk form, used when a panel first subscribes: one message for every named
// channel instead of one per channel.
export function onChannelDisplayNamesChanged(names, ownerKey) {
  const callbacks = ownerKey ? vmCallbacksByOwner.get(ownerKey) : vmCallbacks;
  if (ownerKey && !callbacks) return;
  callbacks?.onChannelDisplayNamesChanged?.(names);
}

export function onExternalEvent(event) {
  if (vmCallbacks?.onExternalEvent) {
    vmCallbacks.onExternalEvent(event);
  } else {
    console.log('externalEvent:', event);
  }
}

export function onFlashMemberLoaded(spriteNum, castLib, castMember, swfData, width, height, pausedAtStart, assertedFrame, ownerKey) {
  const callbacks = ownerKey ? vmCallbacksByOwner.get(ownerKey) : vmCallbacks;
  if (ownerKey && !callbacks) return;
  if (callbacks?.onFlashMemberLoaded) {
    callbacks.onFlashMemberLoaded(spriteNum, castLib, castMember, swfData, width, height, pausedAtStart, assertedFrame, ownerKey);
  } else {
    console.log('Flash member loaded:', 'sprite#' + spriteNum, castLib, castMember, width, height, swfData.length, 'bytes', 'pausedAtStart=' + pausedAtStart, 'assertedFrame=' + assertedFrame);
  }
}

export function onFlashMemberUnloaded(spriteNum, ownerKey) {
  const callbacks = ownerKey ? vmCallbacksByOwner.get(ownerKey) : vmCallbacks;
  if (ownerKey && !callbacks) return;
  if (callbacks?.onFlashMemberUnloaded) {
    callbacks.onFlashMemberUnloaded(spriteNum, ownerKey);
  } else {
    console.log('Flash member unloaded: sprite#' + spriteNum);
  }
}

export function onFlashResetAll(ownerKey) {
  const callbacks = ownerKey ? vmCallbacksByOwner.get(ownerKey) : vmCallbacks;
  if (ownerKey && !callbacks) return;
  if (callbacks?.onFlashResetAll) {
    callbacks.onFlashResetAll(ownerKey);
  }
}

export function onFlashGetVariable(spriteNum, path, ownerKey) {
  return dispatchVmCallback(ownerKey, 'onFlashGetVariable', spriteNum, path);
}
export function onFlashSetVariable(spriteNum, path, value, ownerKey) {
  return dispatchVmCallback(ownerKey, 'onFlashSetVariable', spriteNum, path, value);
}
export function onFlashCallFunction(spriteNum, path, argsXml, ownerKey) {
  return dispatchVmCallback(ownerKey, 'onFlashCallFunction', spriteNum, path, argsXml);
}
export function onFlashGotoFrame(spriteNum, frameOrLabel, ownerKey) {
  return dispatchVmCallback(ownerKey, 'onFlashGotoFrame', spriteNum, frameOrLabel);
}
export function onFlashGotoFrameAndStop(spriteNum, frameOrLabel, ownerKey) {
  return dispatchVmCallback(ownerKey, 'onFlashGotoFrameAndStop', spriteNum, frameOrLabel);
}
export function onFlashInstanceReady(spriteNum, ownerKey) {
  return dispatchVmCallback(ownerKey, 'onFlashInstanceReady', spriteNum);
}

export function onStageSizeChanged(width, height, center) {
  if (vmCallbacks?.onStageSizeChanged) {
    vmCallbacks.onStageSizeChanged(width, height, center);
  }
}

// ─────────────────────────────────────────────────────────────────────
// External Xtra plugin loader + JS↔WASM bridge
//
// Works in every dirplayer-rs host: the dev React app, the polyfill
// bundle, the browser extension, and Electron. Each host calls
// `loadExternalXtra(url)` for whichever plugins it wants to load; the
// rest happens here. No host-specific code.
//
// Architecture:
//   plugin .wasm   <— this module —>   vm-rust wasm
//        ↑                                  ↑
//        │ dx_host_call                     │ external_xtra_host_dispatch
//        │                                  │
//   plugin imports                  vm-rust exports
//
// Postcard encoding is entirely on the Rust side. JS just shuffles
// bytes between plugin memory and vm-rust calls; it does not decode
// the wire format.

// Plugins are capabilities of one exact BrowserPlayerHandle generation.
const _plugins = new Map(); // `${ownerKey}\0${lowercaseName}` -> plugin slot
const _externalXtraHostsByOwner = new Map(); // owner key -> BrowserPlayerHandle
let _vmModule = null;       // lazy-loaded `vm-rust` module reference
let _xtraMovieBase = null;  // base URL used for movie-relative xtra resolution
let _xtraHostBase = null;   // base URL for "~/foo.wasm" — points at where the host JS lives
const _xtraRegistry = new Map(); // normalized-name -> url (movie XTRl resolves through here)

/// Normalize a registry/movie xtra name to its lookup key:
///   - lowercased
///   - ".x32" / ".x16" / ".xtr" extension stripped
/// So "BobbaXtra", "bobbaxtra", "BobbaXtra.x32" all key to "bobbaxtra".
function _normalizeXtraKey(name) {
  if (typeof name !== 'string') return '';
  let s = name.toLowerCase();
  s = s.replace(/\.(?:x32|x16|xtr|wasm)$/i, '');
  return s;
}

function _pluginKey(ownerKey, name) {
  return `${ownerKey || ''}\0${_normalizeXtraKey(name)}`;
}

function _pluginFor(ownerKey, name) {
  return _plugins.get(_pluginKey(ownerKey, name));
}

/// Derive a "by convention" URL for an xtra name. Used as the last-resort
/// fallback when the registry (JSON file + localStorage override) has no
/// entry for `name`. Strategy: strip the extension, split camelCase into
/// snake_case, and serve `~/<snake_case>.wasm` — `~/` resolves through
/// `setXtraHostBase` so each host environment finds the wasm in its own
/// "xtras live here" directory:
///
///   "BobbaXtra"     → "~/bobba_xtra.wasm"
///   "BobbaXtra.x32" → "~/bobba_xtra.wasm"
///   "OpenURL"       → "~/open_url.wasm"      (initialism collapse)
///   "Multiusr"      → "~/multiusr.wasm"      (no internal upper)
///
/// Host bases:
///   - dev:       document.baseURI (set by VMProvider)
///   - polyfill:  <polyfill-script-base>     (set by standalone.tsx)
///   - extension: chrome-extension://<id>/xtras/  (set by content-script.tsx)
///   - electron:  document.baseURI (same as dev — Electron uses VMProvider)
///
/// Hosts that ship xtras under non-conventional filenames pin them
/// explicitly in xtra-registry.json. This fallback covers the common
/// case of "I dropped foo_xtra.wasm in the host's xtras dir and want
/// it to just work."
function _conventionUrl(name) {
  if (typeof name !== 'string' || !name) return null;
  let n = name.replace(/\.(?:x32|x16|xtr|wasm)$/i, '');
  n = n
    // Insert _ between a lowercase/digit and an uppercase (BobbaXtra → Bobba_Xtra).
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    // Insert _ inside a run of uppercase followed by a lowercase (HTMLParser → HTML_Parser).
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .toLowerCase();
  return `~/${n}.wasm`;
}

/// Fetch a registry JSON file and merge its entries into the registry.
/// Each host bootstrap calls this after setting its own
/// `setXtraHostBase(...)`. Missing or malformed file is non-fatal —
/// the convention fallback (`~/<snake>.wasm`) still works.
///
/// `path` defaults to `~/xtra-registry.json` (host-base relative).
/// Pass another path (any form `_resolveXtraUrl` understands —
/// `~/...`, `/...`, `https://...`, bare movie-relative) to point
/// elsewhere. Each host has its own override hook layered on top:
///   - polyfill: `data-xtra-registry-url` script attribute
///   - dev/electron/extension: pass programmatically if needed
///
/// Returns a Promise that resolves to the parsed map (or null on miss),
/// so hosts can await this before issuing the first movie load.
export async function loadDefaultXtraRegistry(path = '~/xtra-registry.json') {
  let url;
  try {
    url = _resolveXtraUrl(path);
  } catch (e) {
    // No host base set when the path needs one (e.g. "~/..."). Bail
    // quietly; convention fallback will also be unavailable.
    console.warn(`[dirplayer] loadDefaultXtraRegistry: cannot resolve ${path}, skipping`);
    return null;
  }
  try {
    const resp = await fetch(url, { cache: 'no-cache' });
    if (!resp.ok) {
      // 404 is fine — the file is optional.
      return null;
    }
    const map = await resp.json();
    if (!map || typeof map !== 'object' || Array.isArray(map)) return null;
    setXtraRegistry(map);
    const keys = Object.keys(map);
    if (keys.length > 0) {
      console.log(`[dirplayer] xtra registry primed from ${url}: ${keys.join(', ')}`);
    }
    return map;
  } catch (e) {
    console.warn(`[dirplayer] could not load ${url}:`, e);
    return null;
  }
}

/// Set or merge the name→URL registry used to resolve a movie's XTRl
/// declarations. Each host (dev / polyfill / extension / Electron)
/// calls this at boot with whatever its config tells it. Repeated
/// calls MERGE (later wins per key); pass an empty object to clear.
///
/// URL values follow the same resolver rules as loadExternalXtra:
/// "https://..." absolute; "/path" relative to document; bare name
/// relative to current movie (only meaningful at movie-load time).
export function setXtraRegistry(map) {
  if (!map) { _xtraRegistry.clear(); return; }
  for (const [name, url] of Object.entries(map)) {
    if (typeof url !== 'string' || !url) continue;
    _xtraRegistry.set(_normalizeXtraKey(name), url);
  }
}

export function getXtraRegistry() {
  // Defensive copy so callers can't mutate the live map.
  const out = {};
  for (const [k, v] of _xtraRegistry) out[k] = v;
  return out;
}

/// Resolve the currently-loaded movie's XTRl declarations against the
/// registry and load any matched plugins that aren't already loaded.
/// Returns a summary object describing what happened.
export async function resolveAndLoadMovieXtras(hostHandle) {
  const vm = _getVmModule();
  if (typeof vm.movie_required_xtras !== 'function') {
    return { skipped: [], loaded: [], failed: [], missing: [] };
  }
  const required = vm.movie_required_xtras(); // Array of { filename, displayName }
  const ownerKey = hostHandle ? hostHandle.owner_identity() : '';
  const skipped = [];
  const missing = [];
  const toLoad = [];
  for (const entry of required) {
    const filename = entry.filename || '';
    const display = entry.displayName || '';
    const key = _normalizeXtraKey(display || filename);
    if (_pluginFor(ownerKey, key) || _pluginFor(ownerKey, filename)) {
      // Already loaded under either key form.
      skipped.push(display || filename);
      continue;
    }
    // Try display-name key first, then filename stem. Convention
    // fallback (`~/<snake>.wasm`) is intentionally NOT consulted here:
    // an average movie's XTRl declares Director's built-in xtras
    // (Multiusr, Font Xtra, Shockwave 3D Asset, INETURL, ...) which
    // the host handles natively. Speculatively fetching every one of
    // those as a wasm would 404 the lot and dump 10+ failures into the
    // console on every movie load.
    //
    // Convention fallback DOES fire from `onRequestXtraLoad` — that
    // path runs only when Lingo *explicitly* references an unknown
    // xtra by name, so a 404 there is a genuine "movie expected a
    // plugin we don't have" signal worth surfacing.
    const url =
      _xtraRegistry.get(key) ||
      _xtraRegistry.get(_normalizeXtraKey(filename));
    if (!url) {
      missing.push({ filename, displayName: display });
      continue;
    }
    toLoad.push({ name: display || filename, url });
  }
  const loadResults = await Promise.allSettled(
    toLoad.map((t) => loadExternalXtra(t.url, hostHandle).then((name) => ({ t, name })))
  );
  const loaded = [];
  const failed = [];
  for (let i = 0; i < loadResults.length; i++) {
    const r = loadResults[i];
    if (r.status === 'fulfilled') {
      loaded.push(r.value.name);
    } else {
      failed.push({ name: toLoad[i].name, url: toLoad[i].url, error: String(r.reason) });
    }
  }
  // `failed` = real load error (registry hit a URL, fetch/instantiate
  // blew up). Worth a console.warn.
  //
  // `missing` = XTRl declared an xtra with no registry match. For most
  // movies this is normal — the XTRl lists Director's built-in xtras
  // (Multiusr, Font Xtra, Shockwave 3D Asset, INETURL, ...) which the
  // host handles natively without a wasm plugin. Surfacing those as a
  // yellow warning every movie load is just noise; demote to debug so
  // it shows under DevTools' "Verbose" level when triaging a movie
  // that actually needs a plugin loaded.
  if (failed.length) {
    console.warn('[dirplayer] resolveAndLoadMovieXtras: failed to load xtras', { loaded, failed, missing });
  } else if (missing.length) {
    console.debug('[dirplayer] resolveAndLoadMovieXtras:', { loaded, skipped, missing });
  }
  return { skipped, loaded, failed, missing };
}

/// Set the base URL that bare xtra filenames resolve against. Host code
/// (LoadMovie, EmbedPlayer, polyfill bootstrap, extension entry) should
/// call this immediately before loading a movie so any subsequent
/// loadExternalXtra("bare.wasm") resolves to "<movieBase>/bare.wasm".
///
/// Resolution rules in loadExternalXtra:
///   - "http://..." / "https://..." / "chrome-extension://..." → as-is
///   - "/anything.wasm"                  → relative to document.baseURI
///   - "~/anything.wasm"                 → relative to host base (setXtraHostBase)
///   - "anything.wasm"  (bare)           → relative to the current movie base
export function setXtraMovieBase(base) {
  _xtraMovieBase = base || null;
}

/// Set the base URL used by the "~/..." prefix in xtra URLs. Useful in
/// the polyfill / extension cases where the host JS lives at a known
/// CDN location and wants to serve its xtras alongside itself, without
/// requiring them to be co-located with the .dcr movie files.
///
/// Polyfill `standalone.tsx` calls this with `<polyfill-script-base>`
/// at init; extension / Electron hosts can call it with whatever base
/// makes sense (e.g. `chrome-extension://<id>/xtras/`).
export function setXtraHostBase(base) {
  _xtraHostBase = base || null;
}

/// Current host base (or `null` if none set). Used by host bootstraps
/// that may run nested — e.g. the polyfill's `standalone.tsx` configures
/// the host base, then mounts the React app whose `VMProvider` would
/// otherwise unconditionally call `setXtraHostBase(document.baseURI)`
/// and clobber the polyfill's setting. VMProvider checks this getter
/// first and skips its own setup when an outer host already configured
/// things.
export function getXtraHostBase() {
  return _xtraHostBase;
}

function _resolveXtraUrl(url) {
  // Already an absolute URL (any scheme)? Use as-is.
  try {
    return new URL(url).href;
  } catch {
    /* relative — fall through */
  }
  // Host-base prefix "~/foo.wasm" → resolve against the host JS location.
  if (url.startsWith('~/')) {
    if (!_xtraHostBase) {
      throw new Error(
        `loadExternalXtra: cannot resolve '${url}' — no host base set. ` +
        `Call setXtraHostBase(...) from the host's bootstrap (e.g. ` +
        `polyfill standalone init) with the URL the host JS was loaded from.`
      );
    }
    const absHost = new URL(_xtraHostBase, document.baseURI).href;
    return new URL(url.substring(2), absHost).href;
  }
  // Absolute path → resolve against the document.
  if (url.startsWith('/')) {
    return new URL(url, document.baseURI).href;
  }
  // Bare filename → resolve against the current movie base.
  if (!_xtraMovieBase) {
    throw new Error(
      `loadExternalXtra: cannot resolve '${url}' — no movie base set. ` +
      `Bare filenames resolve against the current movie path; call ` +
      `setXtraMovieBase(...) first, OR use a leading '/' for paths ` +
      `relative to the host document, OR '~/' for paths relative to ` +
      `the host JS location, OR a full 'http(s)://' URL.`
    );
  }
  // Absolutize the movie base against the document to handle path-only
  // bases like "/movies/foo/" before resolving the bare filename.
  const absBase = new URL(_xtraMovieBase, document.baseURI).href;
  return new URL(url, absBase).href;
}

// Some Rust toolchains use big-endian for the (ptr<<32)|len pack; ours
// (wasm32-unknown-unknown release) uses little-endian. We construct the
// u64 the same way the SDK does in `abi::pack`.
function _packPtr(ptr, len) {
  return (BigInt(ptr) << 32n) | BigInt(len);
}
function _unpackPtr(packed) {
  return [Number(packed >> 32n), Number(packed & 0xFFFFFFFFn)];
}

function _writePluginBytes(plugin, bytes) {
  if (bytes.length === 0) return 0;
  const ptr = plugin.exports.__plugin_alloc(bytes.length);
  new Uint8Array(plugin.exports.memory.buffer, ptr, bytes.length).set(bytes);
  return ptr;
}

function _readPluginBytes(plugin, ptr, len) {
  if (ptr === 0 || len === 0) return new Uint8Array(0);
  // Copy out: the plugin may dealloc the source buffer immediately after.
  return new Uint8Array(plugin.exports.memory.buffer, ptr, len).slice();
}

function _readPackedAndDealloc(plugin, packed) {
  const [ptr, len] = _unpackPtr(packed);
  if (ptr === 0) return new Uint8Array(0);
  const out = _readPluginBytes(plugin, ptr, len);
  plugin.exports.__plugin_dealloc(ptr, len);
  return out;
}

/// Explicitly hand the bridge the vm-rust module reference. Hosts that
/// load this bridge through a bundler with CommonJS interop (webpack /
/// Vite — dev, polyfill, extension, Electron) don't need to call this;
/// the lazy `require('vm-rust')` fallback below picks it up. Hosts that
/// run in pure-ESM contexts WITHOUT CommonJS resolution (the browser
/// e2e test harness uses an importmap) MUST call this once at boot
/// after `await init()` so plugin dispatch and on-demand loading can
/// route back into vm-rust.
export function setVmModule(mod) {
  _vmModule = mod;
}

function _getVmModule() {
  if (_vmModule) return _vmModule;
  // Bundler fallback. webpack/Vite rewrite this to their own resolver;
  // in environments without one (raw importmap, deno, etc.) `require`
  // is undefined and the call below throws — the caller should have
  // wired the module via `setVmModule(...)` at host bootstrap instead.
  if (typeof require === 'function') {
    _vmModule = require('vm-rust');
    return _vmModule;
  }
  throw new Error(
    'dirplayer-js-api: vm-rust module not wired. Call setVmModule(wasm) ' +
    'after awaiting the vm-rust init() promise before any plugin operation.'
  );
}

// Tracks the in-flight load of every URL ever passed to loadExternalXtra,
// so a movie can `await getExternalXtrasReady()` before evaluating
// scripts that reference an external xtra. Resolves to `null` when no
// loads have been initiated, otherwise to a Promise<string[]> of names.
const _pendingLoads = [];

/// Load every URL in `urls` in parallel. Resolves with the array of
/// loaded xtra names (same order as input). Hosts call this at boot
/// with whichever URLs they want available (dev=localStorage list,
/// polyfill=init-script, extension=chrome.storage, Electron=app config).
export function loadExternalXtras(urls, hostHandle) {
  if (!urls || urls.length === 0) return Promise.resolve([]);
  // Each loadExternalXtra() already pushes itself to _pendingLoads.
  return Promise.all(urls.map((u) => loadExternalXtra(u, hostHandle)));
}

/** Register the owner-bound host used by on-demand Xtra loads. */
export function registerExternalXtraHost(handle) {
  let ownerKey = handle.owner_identity();
  _externalXtraHostsByOwner.set(ownerKey, handle);
  const registration = (() => {
    if (_externalXtraHostsByOwner.get(ownerKey) === handle) {
      _externalXtraHostsByOwner.delete(ownerKey);
    }
  });
  registration.rebindOwner = () => {
    if (_externalXtraHostsByOwner.get(ownerKey) === handle) {
      _externalXtraHostsByOwner.delete(ownerKey);
    }
    ownerKey = handle.owner_identity();
    _externalXtraHostsByOwner.set(ownerKey, handle);
  };
  return registration;
}

function _disposeExternalXtraOwner(ownerKey) {
  if (!ownerKey) return;
  if (_externalXtraHostsByOwner.get(ownerKey)) {
    _externalXtraHostsByOwner.delete(ownerKey);
  }
  const prefix = `${ownerKey}\0`;
  for (const key of _plugins.keys()) {
    if (key.startsWith(prefix)) _plugins.delete(key);
  }
  for (const key of _onDemandInFlight) {
    if (key.startsWith(prefix)) _onDemandInFlight.delete(key);
  }
}

export function disposeExternalXtraHost(ownerKey) {
  _disposeExternalXtraOwner(ownerKey);
}

/// Resolves once every loadExternalXtra/s call initiated so far has
/// finished (either resolved or rejected — does not throw on failure).
/// Movie-loading code can await this to ensure plugin-using scripts see
/// their xtras registered. Returns immediately if no loads are pending.
export async function getExternalXtrasReady() {
  if (_pendingLoads.length === 0) return;
  await Promise.allSettled(_pendingLoads.slice());
}

/// Public API. Fetches a plugin .wasm from the URL, instantiates it
/// with the `dirplayer_xtra_host::dx_host_call` import wired to vm-rust,
/// and registers the resulting xtra by name. The returned Promise
/// resolves with the xtra name (so the host can confirm which plugin
/// loaded). The promise is also tracked by `getExternalXtrasReady`
/// regardless of whether you await it directly.
export function loadExternalXtra(url, hostHandle) {
  const p = _loadExternalXtraInner(url, hostHandle);
  _pendingLoads.push(p);
  return p;
}

async function _loadExternalXtraInner(url, hostHandle) {
  const resolved = _resolveXtraUrl(url);
  const ownerKey = hostHandle ? hostHandle.owner_identity() : '';
  const wasmBytes = await fetch(resolved).then((r) => {
    if (!r.ok) throw new Error(`loadExternalXtra: HTTP ${r.status} for ${resolved}`);
    return r.arrayBuffer();
  });

  // The plugin is instantiated BEFORE we know its name. We need the
  // import satisfied at this point, but we don't have a `plugin` object
  // to read memory from yet — patch it after instantiation.
  const pluginSlot = { exports: null };
  const imports = {
    dirplayer_xtra_host: {
      dx_host_call: (opId, argsPtr, argsLen) => {
        if (!pluginSlot.exports) return 0n;
        const argsBytes = _readPluginBytes(pluginSlot, argsPtr, argsLen);
        if (hostHandle && hostHandle.owner_identity() !== ownerKey) {
          throw new Error('loadExternalXtra: owner retired during plugin host call');
        }
        if (!hostHandle || typeof hostHandle.external_xtra_host_dispatch !== 'function') {
          throw new Error('loadExternalXtra: owner-bound host handle is required');
        }
        const dispatch = hostHandle.external_xtra_host_dispatch.bind(hostHandle);
        const result = dispatch(opId, argsBytes);
        // result is Uint8Array (possibly empty for void sentinel).
        if (!result || result.length === 0) return 0n;
        const ptr = _writePluginBytes(pluginSlot, result);
        return _packPtr(ptr, result.length);
      },
    },
  };

  const { instance } = await WebAssembly.instantiate(wasmBytes, imports);
  if (hostHandle && hostHandle.owner_identity() !== ownerKey) {
    throw new Error('loadExternalXtra: owner retired during plugin instantiation');
  }
  pluginSlot.exports = instance.exports;

  // Read the xtra name out of the plugin.
  const namePacked = instance.exports.__xtra_name();
  const nameBytes = _readPackedAndDealloc(pluginSlot, namePacked);
  const name = new TextDecoder().decode(nameBytes);
  if (!name) throw new Error(`loadExternalXtra(${url}): plugin returned empty xtra name`);

  pluginSlot.ownerKey = ownerKey;
  _plugins.set(_pluginKey(ownerKey, name), pluginSlot);

  // Register with this exact owner. A plugin loaded by one provider must
  // never become visible to another provider's player.
  if (!hostHandle || typeof hostHandle.register_external_xtra !== 'function') {
    throw new Error(`loadExternalXtra(${url}): owner-bound registration is unavailable`);
  }
  hostHandle.register_external_xtra(name);

  return name;
}

// ─── Bridge functions called by vm-rust extern declarations ──────────

export function dispatchExternalXtraStaticHandler(xtraName, handler, args, ownerKey) {
  const plugin = _pluginFor(ownerKey, xtraName);
  if (!plugin) return undefined;

  const handlerBytes = new TextEncoder().encode(handler);
  const handlerPtr = _writePluginBytes(plugin, handlerBytes);
  const argsPtr = _writePluginBytes(plugin, args);
  const packed = plugin.exports.__xtra_call_static_handler(
    handlerPtr, handlerBytes.length, argsPtr, args.length,
  );
  const result = _readPackedAndDealloc(plugin, packed);
  plugin.exports.__plugin_dealloc(handlerPtr, handlerBytes.length);
  plugin.exports.__plugin_dealloc(argsPtr, args.length);
  return result;
}

export function dispatchExternalXtraInstanceHandler(xtraName, instanceId, handler, args, ownerKey) {
  const plugin = _pluginFor(ownerKey, xtraName);
  if (!plugin) return undefined;

  const handlerBytes = new TextEncoder().encode(handler);
  const handlerPtr = _writePluginBytes(plugin, handlerBytes);
  const argsPtr = _writePluginBytes(plugin, args);
  const packed = plugin.exports.__xtra_call_handler(
    instanceId, handlerPtr, handlerBytes.length, argsPtr, args.length,
  );
  const result = _readPackedAndDealloc(plugin, packed);
  plugin.exports.__plugin_dealloc(handlerPtr, handlerBytes.length);
  plugin.exports.__plugin_dealloc(argsPtr, args.length);
  return result;
}

export function createExternalXtraInstance(xtraName, args, ownerKey) {
  const plugin = _pluginFor(ownerKey, xtraName);
  if (!plugin) return undefined;

  const argsPtr = _writePluginBytes(plugin, args);
  const packed = plugin.exports.__xtra_create_instance(argsPtr, args.length);
  const result = _readPackedAndDealloc(plugin, packed);
  plugin.exports.__plugin_dealloc(argsPtr, args.length);
  return result;
}

export function destroyExternalXtraInstance(xtraName, instanceId, ownerKey) {
  const plugin = _pluginFor(ownerKey, xtraName);
  if (!plugin) return;
  plugin.exports.__xtra_destroy_instance(instanceId);
}

export function externalXtraHasStaticHandler(xtraName, handler, ownerKey) {
  const plugin = _pluginFor(ownerKey, xtraName);
  if (!plugin) return 0;
  const handlerBytes = new TextEncoder().encode(handler);
  const handlerPtr = _writePluginBytes(plugin, handlerBytes);
  const has = plugin.exports.__xtra_has_static_handler(handlerPtr, handlerBytes.length);
  plugin.exports.__plugin_dealloc(handlerPtr, handlerBytes.length);
  return has;
}

// ── On-demand load callback (called by vm-rust on unknown xtra) ───────
//
// vm-rust fires `onRequestXtraLoad(name, ownerKey, capability)` from
// `request_xtra_load` when
// Lingo executes `new(xtra "name")` for a name that isn't registered.
// We resolve the name through the registry, load the .wasm if matched,
// then call `complete_external_xtra_load(name, capability, success)` back into
// vm-rust so the parked bytecode handler can resume.
//
// One name → at most one in-flight load. Repeat triggers (e.g. multiple
// concurrent waiters from inside vm-rust) are coalesced on the Rust side
// already (only the first call into here fires for a given name), but
// we still guard against a second call from a different source firing
// while a load is in flight.
const _onDemandInFlight = new Set();

export function onRequestXtraLoad(name, ownerKey, capability) {
  const hostHandle = ownerKey ? _externalXtraHostsByOwner.get(ownerKey) : undefined;
  if (ownerKey && !hostHandle) {
    _completeOnDemandLoad(name, ownerKey, capability, false);
    return;
  }
  const key = (name || '').toLowerCase();
  const ownerLoadKey = `${ownerKey || ''}\0${key}`;
  if (!key) {
    _completeOnDemandLoad(name, ownerKey, capability, false);
    return;
  }
  if (_pluginFor(ownerKey, key)) {
    // Already loaded — signal success immediately. (Shouldn't normally
    // happen, but vm-rust's request_xtra_load fast-paths registered
    // names; this is the defence-in-depth catch.)
    _completeOnDemandLoad(name, ownerKey, capability, true);
    return;
  }
  if (_onDemandInFlight.has(ownerLoadKey)) {
    // Another caller already started this load. The completion handler
    // there will signal all waiters when it finishes.
    return;
  }
  // Resolution order: explicit registry pin first; then the snake_case
  // convention (BobbaXtra → /bobba_xtra.wasm). This lets a clean profile
  // pick up wasms dropped in public/ without any localStorage seeding.
  const url =
    _xtraRegistry.get(_normalizeXtraKey(key)) ||
    _conventionUrl(name);
  if (!url) {
    console.warn(`[dirplayer] onRequestXtraLoad: no registry entry or convention URL for '${name}'`);
    _completeOnDemandLoad(name, ownerKey, capability, false);
    return;
  }
  _onDemandInFlight.add(ownerLoadKey);
  loadExternalXtra(url, hostHandle)
    .then((loadedName) => {
      _onDemandInFlight.delete(ownerLoadKey);
      console.log(`[dirplayer] on-demand loaded '${loadedName}' from ${url}`);
      _completeOnDemandLoad(name, ownerKey, capability, true);
    })
    .catch((err) => {
      _onDemandInFlight.delete(ownerLoadKey);
      console.error(`[dirplayer] on-demand load failed for '${name}' (${url}):`, err);
      _completeOnDemandLoad(name, ownerKey, capability, false);
    });
}

function _completeOnDemandLoad(name, ownerKey, capability, success) {
  try {
    const hostHandle = ownerKey ? _externalXtraHostsByOwner.get(ownerKey) : undefined;
    if (!hostHandle || typeof hostHandle.complete_external_xtra_load !== 'function') {
      // The owner may have been retired while the fetch was in flight; its
      // player canceled the waiter, so there is no valid completion target.
      return;
    }
    hostHandle.complete_external_xtra_load(name, capability, success);
  } catch (e) {
    console.error('[dirplayer] complete_external_xtra_load threw:', e);
  }
}
