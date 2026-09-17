import {
  useEffect,
  createContext,
  useReducer,
  useContext,
} from "react";
import init from "vm-rust";
import * as wasm from "vm-rust";
import { initVmCallbacks } from "../vm/callbacks";
import type { VmCallbackRegistration } from "../vm/callbacks";
import {
  JsBridgeBreakpoint,
  getXtraHostBase,
  loadDefaultXtraRegistry,
  loadExternalXtras,
  setVmModule,
  setXtraHostBase,
  setXtraRegistry,
} from "dirplayer-js-api";
import { getFullPathFromOrigin } from "../utils/path";
import { initAudioContext, initAudioBackend } from "../audio/audioInit";
import { useDispatch } from "react-redux";
import { ready } from "../store/vmSlice";
import { isElectron } from "../utils/electron";
import { initializeNetLoader } from "../services/netLoader";
import { initMcpServer, isMcpEnabled } from "../mcp";

interface VMProviderProps {
  children?: string | JSX.Element | JSX.Element[];
  systemFontPath?: string; // Optional override for system font path (used in extension)
  wasmUrl?: string; // Optional override for WASM URL (used in extension)
}

interface PlayerVMState {
  isLoading: boolean;
  browserHandle: wasm.BrowserPlayerHandle | null;
  resetPlayer: (() => void) | null;
}

interface PlayerVMStateAction {
  type: "INIT_OK";
  browserHandle: wasm.BrowserPlayerHandle;
  resetPlayer: () => void;
}

const defaultPlayerState: PlayerVMState = {
  isLoading: true,
  browserHandle: null,
  resetPlayer: null,
};

function playerVmReducer(
  state: PlayerVMState,
  action: PlayerVMStateAction,
): PlayerVMState {
  switch (action.type) {
    case "INIT_OK":
      return {
        ...state,
        isLoading: false,
        browserHandle: action.browserHandle,
        resetPlayer: action.resetPlayer,
      };
  }
}

export const VMProviderContext =
  createContext<PlayerVMState>(defaultPlayerState);

// The WASM module + global VM setup must run EXACTLY ONCE per page, even
// when several VMProvider instances mount. The polyfill renders a separate
// React root + VMProvider per movie embed; wasm-bindgen's init() re-runs
// instantiation on every call, which drops the previous run's in-flight
// async closures (e.g. the system-font load) and throws "closure invoked
// recursively or after being dropped" — leaving the system font never
// loaded. A shared module-level promise gates it: the first caller runs the
// setup and the rest await the same result.
let vmGlobalInitPromise: Promise<void> | null = null;

function ensureVmGlobalInit(wasmUrl?: string): Promise<void> {
  if (vmGlobalInitPromise) return vmGlobalInitPromise;
  vmGlobalInitPromise = (async () => {
    // Step 1: Initialize AudioContext (required before WASM init)
    initAudioContext();

    // Step 2: Initialize WASM and VM
    if (wasmUrl) {
      await init({ module_or_path: wasmUrl });
    } else {
      await init({});
    }
    console.log("VM initialized");
    // Hand the wasm module to the xtra bridge. The bridge's lazy
    // `require('vm-rust')` fallback only works under bundlers that emit
    // CommonJS interop at runtime (CRA's webpack does); the polyfill IIFE
    // bundle and the extension content script have no `require` at runtime
    // and would otherwise throw "vm-rust module not wired" on the first
    // plugin op. Calling setVmModule explicitly works under every host.
    setVmModule(wasm);
    // Auto-load external xtras / merge the xtra registry. Hosts that set
    // their own xtra host base (polyfill / extension) before mounting are
    // left untouched so we don't clobber their origin (which would trigger
    // CORS errors on ~/xtra-registry.json).
    if (!getXtraHostBase()) {
      setXtraHostBase(document.baseURI);
      await loadDefaultXtraRegistry();
    }
    try {
      const rawRegistry = localStorage.getItem("dirplayer_xtra_registry");
      if (rawRegistry) {
        const map = JSON.parse(rawRegistry) as Record<string, string>;
        if (map && typeof map === "object") {
          setXtraRegistry(map);
          const keys = Object.keys(map);
          if (keys.length > 0) {
            console.log("[dirplayer] xtra registry override (localStorage):", keys.join(", "));
          }
        }
      }
    } catch (e) {
      console.warn("[dirplayer] could not parse dirplayer_xtra_registry:", e);
    }

  })();
  return vmGlobalInitPromise;
}

export default function VMProvider({ children, systemFontPath, wasmUrl }: VMProviderProps) {
  const dispatch = useDispatch();
  const [vmState, send] = useReducer(playerVmReducer, defaultPlayerState);
  useEffect(() => {
    let cancelled = false;
    let ownedHandle: wasm.BrowserPlayerHandle | null = null;
    let disposeVmCallbacks: VmCallbackRegistration | null = null;
    let disposeNetLoader: (() => void) | null = null;
    let disposed = false;
    let initializationPending = false;
    let freeWhenInitialized = false;

    // All asynchronous initialization paths converge here. React may unmount
    // while the system-font request is in flight, so cleanup must be
    // idempotent across that continuation and the effect teardown.
    const disposeOwnedHandle = () => {
      if (disposed) return;
      disposed = true;
      disposeVmCallbacks?.();
      disposeVmCallbacks = null;
      const handle = ownedHandle;
      ownedHandle = null;
      if (initializationPending) {
        // Keep the Electron loader alive until the in-flight font request
        // settles. Removing it here would discard the response and leave the
        // command future (and therefore the handle) permanently pending.
        freeWhenInitialized = true;
      } else {
        disposeNetLoader?.();
        disposeNetLoader = null;
        (handle as any)?.free?.();
      }
    };

    // Global WASM/VM setup runs once per page (see ensureVmGlobalInit);
    // every instance just awaits it and then marks its own state ready.
    ensureVmGlobalInit(wasmUrl)
      .then(async () => {
        if (cancelled) return;
        const handle = new wasm.BrowserPlayerHandle();
        ownedHandle = handle;
        if (cancelled) {
          disposeOwnedHandle();
          return;
        }
        // Electron file:// requests can be emitted by the font load itself.
        // Install the owner-filtered loader before awaiting that request.
        disposeNetLoader = initializeNetLoader(handle);
        initializationPending = true;
        try {
          await handle.set_system_font_path(systemFontPath || getFullPathFromOrigin("charmap-system.png"));
        } finally {
          initializationPending = false;
          if (disposed) {
            disposeNetLoader?.();
            disposeNetLoader = null;
          }
          if (freeWhenInitialized) {
            freeWhenInitialized = false;
            (handle as any)?.free?.();
          }
        }
        if (cancelled) {
          disposeOwnedHandle();
          return;
        }
        disposeVmCallbacks = initVmCallbacks(handle);
        const savedPfr = window.localStorage.getItem("dirplayer_pfr_enabled");
        if (savedPfr !== null) {
          handle.set_pfr_font_enabled(savedPfr === "true");
        }
        const resetPlayer = () => {
          // Reset rotates the owner generation and clears the Rust subscription
          // flags. Capture the UI's active subscriptions first so the newly
          // registered owner receives fresh snapshots after rebinding.
          const subscriptions = handle.subscription_state();
          handle.reset();
          // Reset rotates the Rust owner generation. Rebind the callback map
          // only after reset succeeds, so stale callbacks remain retired.
          disposeVmCallbacks?.rebindOwner(handle.owner_identity());
          if (subscriptions.score) handle.subscribe_to_score();
          if (subscriptions.channelNames) handle.subscribe_to_channel_names();
        };

        // External xtras must be instantiated with this provider's owner
        // capability; loading them during page-global bootstrap would bind
        // plugin host calls to an arbitrary player.
        try {
          const raw = localStorage.getItem("dirplayer_external_xtras");
          if (raw) {
            const urls = JSON.parse(raw) as string[];
            if (Array.isArray(urls) && urls.length > 0) {
              loadExternalXtras(urls, handle)
                .then((names) =>
                  console.log("[dirplayer] external xtras loaded:", names.join(", ")))
                .catch((e) =>
                  console.error("[dirplayer] external xtra load failed:", e));
            }
          }
        } catch (e) {
          console.warn("[dirplayer] could not parse dirplayer_external_xtras:", e);
        }

        const savedBreakpoints = window.localStorage.getItem("breakpoints");
        if (savedBreakpoints) {
          const breakpoints: JsBridgeBreakpoint[] = JSON.parse(savedBreakpoints);
          for (const bp of breakpoints) {
            handle.add_breakpoint(bp.script_name, bp.handler_name, bp.bytecode_index);
          }
        }
        if (isElectron()) {
          try {
            const mcpServer = initMcpServer(handle, wasm);
            if (isMcpEnabled()) {
              mcpServer.start();
              console.log("MCP server initialized");
            }
          } catch (err) {
            console.warn("Failed to initialize MCP server:", err);
          }
        }
        // Step 7: Mark VM as ready
        send({ type: "INIT_OK", browserHandle: handle, resetPlayer });
        dispatch(ready());
      })
      .catch((err) => {
        disposeOwnedHandle();
        console.error("Failed to initialize VM:", err);
      });

    const initAudioOnUserGesture = () => {
      // Initialize audio backend on first user gesture
      initAudioBackend();
      document.removeEventListener("click", initAudioOnUserGesture);
    };

    // Setup audio initialization on first user gesture (autoplay policy)
    document.addEventListener("click", initAudioOnUserGesture, { once: true });

    return () => {
      cancelled = true;
      disposeOwnedHandle();
      document.removeEventListener("click", initAudioOnUserGesture);
    };
  }, [dispatch, systemFontPath, wasmUrl]);
  return (
    <div>
      {vmState.isLoading && "Loading..."}
      {!vmState.isLoading && (
        <VMProviderContext.Provider value={vmState}>
          {children}
        </VMProviderContext.Provider>
      )}
    </div>
  );
}

export function useVMState(): PlayerVMState {
  return useContext(VMProviderContext);
}

/** Capture the provider's player capability for one render tree. */
export function useVMHandle(): wasm.BrowserPlayerHandle {
  const { browserHandle } = useContext(VMProviderContext);
  if (!browserHandle) {
    throw new Error("VM player handle is not initialized");
  }
  return browserHandle;
}
