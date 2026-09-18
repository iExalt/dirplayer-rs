import {
  ICastMemberRef,
  JsBridgeBreakpoint,
  OnScriptErrorData,
  loadExternalXtra,
  registerExternalXtraHost,
  registerVmCallbacks,
  resolveAndLoadMovieXtras,
  setXtraRegistry,
  getXtraRegistry,
  dispatchVmCallback,
} from "dirplayer-js-api";
import { createBrowserOwnerController, createOwnedBrowserCallbacks, initFlashBridge } from "../services/flashPlayerManager";
import store from "../store";
import { breakpointListChanged, castLibNameChanged, castListChanged, castMemberChanged, castMemberListChanged, channelChanged, channelDisplayNameChanged, channelDisplayNamesChanged, datumSnapshot, debugContentAdded, debugMessageAdded, debugMessagesCleared, frameChanged, globalsChanged, movieLoaded, movieLoadFailed, onScriptError, scopeListChanged, scoreChanged, scriptErrorCleared, scriptInstanceSnapshot } from "../store/vmSlice";
import type { BrowserPlayerHandle, OnMovieLoadedCallbackData } from 'vm-rust'
import type { DebugContent } from "dirplayer-js-api";
import { DatumRef, IVMScope, JsBridgeDatum, MemberSnapshot, ScoreSnapshot, ScoreSpriteSnapshot } from ".";
import { onMemberSelected } from "../store/uiSlice";
import { isUIShown } from "../utils/debug";

const ownerGetVariable = (ownerKey: string, spriteNum: number, path: string) =>
  dispatchVmCallback(ownerKey, 'onFlashGetVariable', spriteNum, path);
const ownerSetVariable = (ownerKey: string, spriteNum: number, path: string, value: string) =>
  dispatchVmCallback(ownerKey, 'onFlashSetVariable', spriteNum, path, value);
const ownerCallFunction = (ownerKey: string, spriteNum: number, path: string, argsXml: string) =>
  dispatchVmCallback(ownerKey, 'onFlashCallFunction', spriteNum, path, argsXml);
const ownerGotoFrame = (ownerKey: string, spriteNum: number, frameOrLabel: string) =>
  dispatchVmCallback(ownerKey, 'onFlashGotoFrame', spriteNum, frameOrLabel);
const ownerGotoFrameAndStop = (ownerKey: string, spriteNum: number, frameOrLabel: string) =>
  dispatchVmCallback(ownerKey, 'onFlashGotoFrameAndStop', spriteNum, frameOrLabel);
const ownerFlashInstanceReady = (ownerKey: string, spriteNum: number) =>
  dispatchVmCallback(ownerKey, 'onFlashInstanceReady', spriteNum);

function installOwnerVmRouters(): void {
  const win = window as any;
  if (typeof win.dirplayer_ruffleGetVariableOwned !== 'function') win.dirplayer_ruffleGetVariableOwned = ownerGetVariable;
  if (typeof win.dirplayer_ruffleSetVariableOwned !== 'function') win.dirplayer_ruffleSetVariableOwned = ownerSetVariable;
  if (typeof win.dirplayer_ruffleCallFunctionOwned !== 'function') win.dirplayer_ruffleCallFunctionOwned = ownerCallFunction;
  if (typeof win.dirplayer_ruffleGoToFrameOwned !== 'function') win.dirplayer_ruffleGoToFrameOwned = ownerGotoFrame;
  if (typeof win.dirplayer_ruffleGoToFrameAndStopOwned !== 'function') win.dirplayer_ruffleGoToFrameAndStopOwned = ownerGotoFrameAndStop;
  if (typeof win.dirplayer_isFlashInstanceReadyOwned !== 'function') win.dirplayer_isFlashInstanceReadyOwned = ownerFlashInstanceReady;
}

export type VmCallbackRegistration = (() => void) & {
  rebindOwner: (ownerKey: string) => void;
};

const nestedController = createBrowserOwnerController((callbacks, ownerKey, setAsDefault) =>
  registerVmCallbacks(callbacks as any, ownerKey, setAsDefault),
);

function installNestedCallbackBridge(): void {
  const win = window as any;
  if (win.dirplayer_registerNestedBrowserOwner) return;
  win.dirplayer_registerNestedBrowserOwner = (
    parentOwnerKey: string,
    childOwnerKey: string,
    capability: any,
  ) => nestedController.registerNested(parentOwnerKey, childOwnerKey, capability);
  win.dirplayer_retireNestedBrowserOwner = (
    parentOwnerKey: string,
    childOwnerKey: string,
  ) => nestedController.retireNested(parentOwnerKey, childOwnerKey);
}

export function initVmCallbacks(browserHandle: BrowserPlayerHandle): VmCallbackRegistration {
  // Initialize the Flash/Ruffle bridge (registers global JS functions for WASM to call)
  let disposeFlashBridge = initFlashBridge(browserHandle);
  let flashHost = disposeFlashBridge.host;
  const disposeExternalXtraHost = registerExternalXtraHost(browserHandle);
  installNestedCallbackBridge();
  const w = window as any;

  // Expose W3D debug tools on window for console access
  (window as any).exportW3dObj = (castLib: number, castMember: number) =>
    browserHandle.export_w3d_obj(castLib, castMember);
  (window as any).exportW3dRaw = (castLib: number, castMember: number) =>
    browserHandle.export_w3d_raw(castLib, castMember);
  (window as any).listW3dMembers = () => browserHandle.list_w3d_members();

  // Expose external xtra loader + registry API so hosts (or devtools)
  // can drive plugin loading interactively. Namespaced with `dirplayer_`
  // so DevTools autocomplete groups them together and they don't
  // collide with anything a host page or extension might already put on
  // window. The dev-environment auto-load that runs at boot lives in
  // VMProvider (after `await init`); these exposures are for ad-hoc
  // testing.
  //
  //   await dirplayer_loadExternalXtra('/example_xtra.wasm')
  //   dirplayer_setXtraRegistry({ BobbaXtra: '~/bobba.wasm' })
  //   await dirplayer_resolveAndLoadMovieXtras()
  //   dirplayer_getXtraRegistry()
  w.dirplayer_loadExternalXtra = (url: string) => loadExternalXtra(url, browserHandle);
  w.dirplayer_setXtraRegistry = setXtraRegistry;
  w.dirplayer_getXtraRegistry = getXtraRegistry;
  w.dirplayer_resolveAndLoadMovieXtras = () => resolveAndLoadMovieXtras(browserHandle);

  // Expose trace log download on window
  (window as any).downloadTraceLog = () => {
    try {
      // Dynamic import to avoid TS type issues before rebuild
      const vm = require('vm-rust');
      const log = vm.get_trace_log?.();
      if (!log) {
        console.log('No trace log available (traceLogFile not set or empty)');
        return;
      }
      const blob = new Blob([log.content], { type: 'text/plain' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      const fileName = log.path.split(/[/\\]/).pop() || 'trace.log';
      a.href = url;
      a.download = fileName;
      a.click();
      URL.revokeObjectURL(url);
      console.log(`Downloaded trace log: ${fileName} (${log.content.length} bytes)`);
    } catch (e) {
      console.error('Failed to download trace log:', e);
    }
  };

  let callbacks = {
    onMovieLoaded: (result: OnMovieLoadedCallbackData) => {
      // Offer trace log download if one was recorded
      try {
        const vm = require('vm-rust');
        const log = vm.get_trace_log?.();
        if (log && log.content.length > 0) {
          const fileName = log.path.split(/[/\\]/).pop() || 'trace.log';
          console.log(`Trace log available: ${fileName} (${log.content.length} bytes) - call downloadTraceLog() to save`);
        }
      } catch {}
      store.dispatch(debugMessagesCleared());
      store.dispatch(movieLoaded());
      // Re-mirror the VM's breakpoints. `movieUnloaded` resets the vm slice to
      // its initial state, which empties the store's copy, but the VM keeps the
      // list it was given (restored from localStorage at boot) and nothing
      // pushes it again. The script gutter therefore came up blank after a
      // movie load even though the breakpoints were live, and only reappeared
      // when the next add/remove finally sent a list.
      store.dispatch(breakpointListChanged(browserHandle.get_breakpoints() as JsBridgeBreakpoint[]));
    },
    onMovieLoadFailed: (path: string, error: string) => {
      store.dispatch(movieLoadFailed(`Failed to load movie: ${error}`));
    },
    onCastListChanged: (castList: string[]) => {
      store.dispatch(castListChanged(castList));
    },
    onCastLibNameChanged: (castNumber: number, name: string) => {
      store.dispatch(castLibNameChanged({ castNumber, name }))
    },
    onCastMemberListChanged: (castNumber: number, members: any) => {
      store.dispatch(castMemberListChanged({ 
        castNumber, 
        members,
      }))
    },
    onCastMemberChanged: (memberRef: ICastMemberRef, snapshot: MemberSnapshot) => {
      store.dispatch(castMemberChanged({ memberRef, snapshot }))
    },
    onFrameChanged: (frame: number) => {
      store.dispatch(frameChanged(frame))
    },
    onScoreChanged: (snapshot: ScoreSnapshot) => {
      store.dispatch(scoreChanged({
        ...snapshot,
      }))
    },
    onScriptError: (errorObj: OnScriptErrorData) => {
      if (!isUIShown()) {
        alert(`Script error: ${errorObj.message}`);
      }
      store.dispatch(onScriptError(errorObj.message))
      store.dispatch(onMemberSelected(errorObj.script_member_ref))
    },
    onScopeListChanged: (scopes: IVMScope[]) => {
      store.dispatch(scopeListChanged(scopes))
    },
    onBreakpointListChanged: (breakpoints: JsBridgeBreakpoint[]) => {
      store.dispatch(breakpointListChanged(breakpoints))
      // Defensive dedup before persisting: guards against the breakpoint list
      // ever ballooning with duplicates (the WASM side also dedups in
      // add_breakpoint). Key on script + handler + bytecode index.
      const seen = new Set<string>()
      const unique = breakpoints.filter((bp) => {
        const k = `${bp.script_name}|${bp.handler_name}|${bp.bytecode_index}`
        if (seen.has(k)) return false
        seen.add(k)
        return true
      })
      window.localStorage.setItem('breakpoints', JSON.stringify(unique))
    },
    onScriptErrorCleared: () => {
      store.dispatch(scriptErrorCleared())
    },
    onGlobalListChanged: (globals: Record<string, any>) => {
      store.dispatch(globalsChanged(globals))
    },
    onDebugMessage: (message: string) => {
      console.log(message);
      store.dispatch(debugMessageAdded(message));
    },
    onDebugContent: (content: DebugContent) => {
      store.dispatch(debugContentAdded(content));
    },
    onDatumSnapshot: (datumRef: DatumRef, datum: JsBridgeDatum) => {
      store.dispatch(datumSnapshot({ datumRef, datum }));
    },
    onScriptInstanceSnapshot: (scriptInstanceId: number, scriptInstance: JsBridgeDatum) => {
      store.dispatch(scriptInstanceSnapshot({ scriptInstanceId, datum: scriptInstance }));
    },
    onChannelChanged: (channelNumber: number, channelData: ScoreSpriteSnapshot) => {
      store.dispatch(channelChanged({ channelNumber, channelData }))
    },
    onChannelDisplayNameChanged: (channelNumber: number, displayName: string) => {
      store.dispatch(channelDisplayNameChanged({ channelNumber, displayName }));
    },
    onChannelDisplayNamesChanged: (names: Record<number, string>) => {
      store.dispatch(channelDisplayNamesChanged(names));
    },
    ...createOwnedBrowserCallbacks(flashHost),
    onStageSizeChanged: (width: number, height: number, center: boolean) => {
      const inner = document.getElementById('stage_canvas_container');
      if (inner) {
        inner.style.width = `${width}px`;
        inner.style.height = `${height}px`;
        const outer = inner.parentElement;
        if (outer) {
          outer.dataset.centerStage = center ? 'true' : 'false';
          outer.style.justifyContent = center ? 'center' : 'flex-start';
          outer.style.alignItems = center ? 'center' : 'flex-start';
        }
      }
    },
  };
  installOwnerVmRouters();
  let disposeRegistered = registerVmCallbacks(callbacks, browserHandle.owner_identity());
  const disposeVmCallbacks = (() => {
    nestedController.disposeNestedTree(flashHost);
    disposeFlashBridge();
    disposeExternalXtraHost();
    disposeRegistered();
  }) as VmCallbackRegistration;
  disposeVmCallbacks.rebindOwner = (ownerKey: string) => {
    // Rebind the closure-held Flash host together with VM callbacks. The old
    // generation is disposed before the new bridge is published, so late
    // Ruffle loads cannot attach to a replacement owner.
    nestedController.disposeNestedTree(flashHost);
    disposeFlashBridge();
    disposeFlashBridge = initFlashBridge(browserHandle);
    flashHost = disposeFlashBridge.host;
    disposeExternalXtraHost.rebindOwner();
    disposeRegistered();
    callbacks = { ...callbacks, ...createOwnedBrowserCallbacks(flashHost) };
    disposeRegistered = registerVmCallbacks(callbacks, ownerKey);
  };
  return disposeVmCallbacks;
}
