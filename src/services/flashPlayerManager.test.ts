import {
  FlashPendingQueue,
  PendingOp,
  createFlashOwnerController,
  deliverOwnedLingoCallback,
  publishFlashInstanceForOwner,
  registerLingoCallbackForOwner,
  dispatchMouseEventForOwnerAtGeneration,
  getSpriteVariableForOwnerAtGeneration,
  registerFlashOwner,
  registerNestedFlashOwner,
  retireNestedFlashOwner,
} from "./flashPlayerManager";
import {
  dispatchVmCallback,
  onFlashMemberLoadedPrepared,
  registerVmCallbacks,
} from "dirplayer-js-api";
import {
  bridgeGetVariableOwnedSync,
  normalizeOwnedFlashVariable,
} from "./ruffleBridgeClient";

const play: PendingOp = { kind: "play" };
const stop: PendingOp = { kind: "stop" };

test("owner-qualified Flash queues replay only at readiness and isolate same sprite numbers", () => {
  const queue = new FlashPendingQueue();

  // Both runtimes queue the same Director sprite before either Ruffle player
  // exists. A readiness drain for A must not consume B's operation.
  queue.enqueueOwned("session-a:player-1", 7, play);
  queue.enqueueOwned("session-b:player-1", 7, stop);

  // Missing/not-ready instances do not drain anything because production only
  // calls drainReady after the corresponding instance is marked ready.
  expect(queue.drainReady(7, "session-a:player-1:7", false, false)).toEqual([]);
  expect(queue.drainReady(7, "session-a:player-1:7", true, false).map(({ op }) => op)).toEqual([play]);
  expect(queue.drainReady(7, "session-b:player-1:7", true, false).map(({ op }) => op)).toEqual([stop]);

  // A legacy queue is consumed only when the sprite index points at the
  // instance becoming ready; an ambiguous owner must leave it queued.
  queue.enqueueLegacy(7, play);
  expect(queue.drainReady(7, "session-c:player-1:7", true, false)).toEqual([]);
  expect(queue.drainReady(7, undefined, true, true).map(({ op }) => op)).toEqual([play]);
});

test("owner retirement clears only that owner's pending Flash operations", () => {
  const queue = new FlashPendingQueue();
  queue.enqueueOwned("session-a:player-1", 7, play);
  queue.enqueueOwned("session-b:player-1", 7, stop);

  queue.clearOwner("session-a:player-1");

  expect(queue.drainReady(7, "session-a:player-1:7", true, false)).toEqual([]);
  expect(queue.drainReady(7, "session-b:player-1:7", true, false).map(({ op }) => op)).toEqual([stop]);
});

function fakeFlashCapability(ownerKey: string) {
  let nextGeneration = 1;
  const current = new Map<number, number>();
  return {
    owner_identity: () => ownerKey,
    reserve_flash_instance_generation: (spriteNum: number) => {
      const generation = nextGeneration++;
      current.set(spriteNum, generation);
      return generation;
    },
    invalidate_flash_instance_generation: (spriteNum: number, generation: number) => {
      if (current.get(spriteNum) !== generation) return false;
      current.delete(spriteNum);
      return true;
    },
    is_flash_instance_generation_current: (spriteNum: number, generation: number) =>
      current.get(spriteNum) === generation,
    set_flash_scripted_access_pending: () => {},
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    trigger_lingo_callback_on_script_ruffle: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
}

function publishTestFlashInstance(
  host: ReturnType<typeof registerFlashOwner>["host"],
  generation: number,
  rufflePlayer: any,
  bridgeId: string | null = null,
): void {
  publishFlashInstanceForOwner(host, {
    host,
    browserHandle: host.capability,
    spriteNum: 1,
    instanceGeneration: generation,
    castLib: 1,
    castMember: 1,
    rufflePlayer,
    bridgeId,
    container: { remove: () => {} } as any,
    canvas: null,
    width: 1,
    height: 1,
    nativeW: 1,
    nativeH: 1,
    animFrameId: null,
    ready: true,
  } as any);
}

function installSyncBridgeResponder(
  resultForPath: (path: string) => unknown,
): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent).detail;
    if (!detail?.sync || detail.method !== "getVariableSync") return;
    const node = document.getElementById("__dirplayer_ruffle_sync_channel");
    if (!node) throw new Error("bridge sync node was not created before request");
    node.textContent = JSON.stringify({
      syncId: detail.syncId,
      result: resultForPath(detail.args?.[0]),
      error: null,
    });
  };
  window.addEventListener("dirplayer-ruffle-bridge-request", listener);
  return () => window.removeEventListener("dirplayer-ruffle-bridge-request", listener);
}

test("sprite bridge getter preserves primitives and only the canonical stored marker", () => {
  const marker = { __dirplayer_stored_path: "_level0.__dirplayer_ref_4294967295" };
  expect(normalizeOwnedFlashVariable("text")).toBe("text");
  expect(normalizeOwnedFlashVariable(7)).toBe(7);
  expect(normalizeOwnedFlashVariable(false)).toBe(false);
  expect(normalizeOwnedFlashVariable(marker)).toBe(marker);
  expect(normalizeOwnedFlashVariable({ __dirplayer_stored_path: "_level0.__dirplayer_ref_4294967296" })).toBeNull();
  expect(normalizeOwnedFlashVariable({ __dirplayer_stored_path: "_level0.__dirplayer_ref_1", extra: true })).toBeNull();
  expect(normalizeOwnedFlashVariable([marker])).toBeNull();
  expect(normalizeOwnedFlashVariable({ value: "object" })).toBeNull();

  const inherited = Object.create({ __dirplayer_stored_path: marker.__dirplayer_stored_path });
  expect(normalizeOwnedFlashVariable(inherited)).toBeNull();
  const accessor = {};
  Object.defineProperty(accessor, "__dirplayer_stored_path", {
    enumerable: true,
    get: () => marker.__dirplayer_stored_path,
  });
  expect(normalizeOwnedFlashVariable(accessor)).toBeNull();

  const stop = installSyncBridgeResponder(path => path === "_root.marker" ? marker : { arbitrary: true });
  expect(bridgeGetVariableOwnedSync("bridge-owner", "_root.marker")).toEqual(marker);
  expect(bridgeGetVariableOwnedSync("bridge-owner", "_root.object")).toBeNull();
  stop();
});

test("sprite getter fences exact owner and generation through direct and bridge routes", () => {
  const rootA = registerFlashOwner("session-sprite-get:a", fakeFlashCapability("session-sprite-get:a"));
  const rootB = registerFlashOwner("session-sprite-get:b", fakeFlashCapability("session-sprite-get:b"));
  const generationA = rootA.host.reserveInstanceGeneration(1);
  const generationB = rootB.host.reserveInstanceGeneration(1);
  const playerA = { GetVariable: () => "A", remove: () => {} };
  const playerB = { GetVariable: () => "B", remove: () => {} };
  publishTestFlashInstance(rootA.host, generationA, playerA);
  publishTestFlashInstance(rootB.host, generationB, playerB);

  expect(getSpriteVariableForOwnerAtGeneration(rootA.host, 1, generationA, "_root.value"))
    .toEqual({ ok: true, generation: generationA, value: "A" });
  expect(getSpriteVariableForOwnerAtGeneration(rootB.host, 1, generationB, "_root.value"))
    .toEqual({ ok: true, generation: generationB, value: "B" });

  rootA.host.invalidateAuthoritativeGeneration(1, generationA);
  expect(getSpriteVariableForOwnerAtGeneration(rootA.host, 1, generationA, "_root.value"))
    .toMatchObject({ ok: false, code: "stale-generation" });
  expect(getSpriteVariableForOwnerAtGeneration(rootB.host, 1, generationB, "_root.value"))
    .toEqual({ ok: true, generation: generationB, value: "B" });

  const bridge = registerFlashOwner("session-sprite-get:bridge", fakeFlashCapability("session-sprite-get:bridge"));
  const bridgeGeneration = bridge.host.reserveInstanceGeneration(1);
  publishTestFlashInstance(bridge.host, bridgeGeneration, { remove: () => {} }, "bridge-sprite-get");
  const stop = installSyncBridgeResponder(() => 17);
  expect(getSpriteVariableForOwnerAtGeneration(bridge.host, 1, bridgeGeneration, "_root.number"))
    .toEqual({ ok: true, generation: bridgeGeneration, value: 17 });
  stop();

  bridge.dispose();
  expect(getSpriteVariableForOwnerAtGeneration(bridge.host, 1, bridgeGeneration, "_root.number"))
    .toMatchObject({ ok: false, code: "disposed-owner" });
  rootA.dispose();
  rootB.dispose();
});

test("sprite getter rejects synchronous replacement during the host read", () => {
  const root = registerFlashOwner("session-sprite-get:reentry", fakeFlashCapability("session-sprite-get:reentry"));
  const generation = root.host.reserveInstanceGeneration(1);
  const player = {
    GetVariable: () => {
      root.host.invalidateAuthoritativeGeneration(1, generation);
      root.host.reserveInstanceGeneration(1);
      return { __dirplayer_stored_path: "_level0.__dirplayer_ref_9" };
    },
    remove: () => {},
  };
  publishTestFlashInstance(root.host, generation, player);

  expect(getSpriteVariableForOwnerAtGeneration(root.host, 1, generation, "_root.object"))
    .toMatchObject({ ok: false, code: "stale-generation" });
  root.dispose();
});

test("nested Flash hosts require the exact parent and isolate overlapping child channels", () => {
  const rootA = registerFlashOwner("session-a:root:1", fakeFlashCapability("session-a:root:1"));
  const rootB = registerFlashOwner("session-b:root:1", fakeFlashCapability("session-b:root:1"));
  const childA = registerNestedFlashOwner(
    "session-a:root:1",
    "session-a:child:1",
    fakeFlashCapability("session-a:child:1"),
  );
  const childB = registerNestedFlashOwner(
    "session-b:root:1",
    "session-b:child:1",
    fakeFlashCapability("session-b:child:1"),
  );

  expect(childA.host.parentHost).toBe(rootA.host);
  expect(childB.host.parentHost).toBe(rootB.host);
  expect(() => retireNestedFlashOwner("session-b:root:1", "session-a:child:1"))
    .toThrow("not owned");
  retireNestedFlashOwner("session-a:root:1", "session-a:child:1");
  expect(childA.host.disposed).toBe(true);
  expect(childB.host.disposed).toBe(false);
  expect(() => retireNestedFlashOwner("session-a:root:1", "session-a:child:1"))
    .not.toThrow();

  rootA.dispose();
  rootB.dispose();
});

test("owner controller isolates two roots with the same local child channel", () => {
  const rootA = registerFlashOwner("session-controller-a:root", fakeFlashCapability("session-controller-a:root"));
  const rootB = registerFlashOwner("session-controller-b:root", fakeFlashCapability("session-controller-b:root"));
  const callbackTables = new Map<string, any>();
  const controller = createFlashOwnerController((callbacks, ownerKey) => {
    callbackTables.set(ownerKey, callbacks);
    return () => {
      if (callbackTables.get(ownerKey) === callbacks) callbackTables.delete(ownerKey);
    };
  });

  controller.registerNested(
    "session-controller-a:root",
    "session-controller-a:child:1",
    fakeFlashCapability("session-controller-a:child:1"),
  );
  controller.registerNested(
    "session-controller-b:root",
    "session-controller-b:child:1",
    fakeFlashCapability("session-controller-b:child:1"),
  );
  expect(callbackTables.size).toBe(2);
  expect(() => controller.retireNested(
    "session-controller-b:root",
    "session-controller-a:child:1",
  )).toThrow("retirement parent mismatch");

  controller.retireNested("session-controller-a:root", "session-controller-a:child:1");
  expect(callbackTables.has("session-controller-a:child:1")).toBe(false);
  expect(callbackTables.has("session-controller-b:child:1")).toBe(true);
  controller.retireNested("session-controller-b:root", "session-controller-b:child:1");
  rootA.dispose();
  rootB.dispose();
});

test("nested owner controller rolls back reentrant registration and retires descendants", () => {
  const root = registerFlashOwner("session-controller:root", fakeFlashCapability("session-controller:root"));
  const callbackTables = new Map<string, any>();
  let reenterRootDispose = true;
  const controller = createFlashOwnerController((callbacks, ownerKey) => {
    callbackTables.set(ownerKey, callbacks);
    if (reenterRootDispose && ownerKey === "session-controller:child") {
      reenterRootDispose = false;
      controller.registerNested(
        "session-controller:child",
        "session-controller:grandchild",
        fakeFlashCapability("session-controller:grandchild"),
      );
      root.dispose();
    }
    return () => {
      if (callbackTables.get(ownerKey) === callbacks) callbackTables.delete(ownerKey);
    };
  });

  expect(() => controller.registerNested(
    "session-controller:root",
    "session-controller:child",
    fakeFlashCapability("session-controller:child"),
  )).toThrow("retired during callback registration");
  expect(callbackTables.has("session-controller:child")).toBe(false);
  expect(callbackTables.has("session-controller:grandchild")).toBe(false);

  const replacementRoot = registerFlashOwner(
    "session-controller:root",
    fakeFlashCapability("session-controller:root"),
  );
  controller.registerNested(
    "session-controller:root",
    "session-controller:child",
    fakeFlashCapability("session-controller:child"),
  );
  expect(callbackTables.has("session-controller:child")).toBe(true);
  controller.retireNested("session-controller:root", "session-controller:child");
  expect(callbackTables.has("session-controller:child")).toBe(false);
  replacementRoot.dispose();
});

test("stale nested callbacks cannot borrow a replacement host", () => {
  const root = registerFlashOwner("session-stale:root", fakeFlashCapability("session-stale:root"));
  const callbacksByOwner = new Map<string, any>();
  const controller = createFlashOwnerController((callbacks, ownerKey) => {
    callbacksByOwner.set(ownerKey, callbacks);
    return () => {
      if (callbacksByOwner.get(ownerKey) === callbacks) callbacksByOwner.delete(ownerKey);
    };
  });

  controller.registerNested(
    "session-stale:root",
    "session-stale:child",
    fakeFlashCapability("session-stale:child"),
  );
  const staleCallbacks = callbacksByOwner.get("session-stale:child");
  controller.retireNested("session-stale:root", "session-stale:child");
  controller.registerNested(
    "session-stale:root",
    "session-stale:child",
    fakeFlashCapability("session-stale:child"),
  );
  const replacementCallbacks = callbacksByOwner.get("session-stale:child");
  expect(replacementCallbacks).not.toBe(staleCallbacks);
  expect(staleCallbacks.onFlashGetVariable(1, "_root.value")).toBeNull();
  expect(replacementCallbacks.onFlashGetVariable(1, "_root.value")).toBeNull();
  controller.retireNested("session-stale:root", "session-stale:child");
  root.dispose();
});

test("owner-qualified mouse forwarding rejects pending and retired generations", () => {
  const root = registerFlashOwner("session-mouse:root", fakeFlashCapability("session-mouse:root"));
  expect(dispatchMouseEventForOwnerAtGeneration(
    root.host, 1, 0, "down", 2, 3, 10, 10,
  )).toMatchObject({ ok: false, code: "invalid-generation" });
  const generation = root.host.reserveInstanceGeneration(1);
  expect(dispatchMouseEventForOwnerAtGeneration(
    root.host, 1, generation, "move", 2, 3, 10, 10,
  )).toMatchObject({ ok: false, code: "not-ready" });
  root.host.invalidateAuthoritativeGeneration(1, generation);
  expect(dispatchMouseEventForOwnerAtGeneration(
    root.host, 1, generation, "down", 2, 3, 10, 10,
  )).toMatchObject({ ok: false });
  root.dispose();
});

test("owned Lingo callback registration uses the exact receiver and strict acknowledgement", () => {
  const rootA = registerFlashOwner("session-lingo:a", fakeFlashCapability("session-lingo:a"));
  const rootB = registerFlashOwner("session-lingo:b", fakeFlashCapability("session-lingo:b"));
  const generationA = rootA.host.reserveInstanceGeneration(7);
  const generationB = rootB.host.reserveInstanceGeneration(7);
  const receiverA = {
    dirplayer_register_lingo_callback: (...args: unknown[]) => {
      expect(args[0]).toBe(7);
      expect(args[1]).toBe(generationA);
      return true;
    },
    remove: () => {},
  };
  const receiverB = {
    dirplayer_register_lingo_callback: () => true,
    remove: () => {},
  };
  const instanceA = {
    host: rootA.host,
    browserHandle: rootA.host.capability,
    spriteNum: 7,
    instanceGeneration: generationA,
    castLib: 1,
    castMember: 1,
    rufflePlayer: receiverA,
    bridgeId: null,
    container: { remove: () => {} } as any,
    canvas: null,
    width: 1,
    height: 1,
    nativeW: 1,
    nativeH: 1,
    animFrameId: null,
    ready: true,
  } as any;
  publishFlashInstanceForOwner(rootA.host, instanceA);
  const instanceB = {
    ...instanceA,
    host: rootB.host,
    browserHandle: rootB.host.capability,
    instanceGeneration: generationB,
    rufflePlayer: receiverB,
  } as any;
  publishFlashInstanceForOwner(rootB.host, instanceB);

  expect(registerLingoCallbackForOwner(rootA.host, 7, generationA, "_root", "onReady", 2, 3, "ready", 4, 5)).toBe(true);
  expect(registerLingoCallbackForOwner(rootB.host, 7, generationB, "_root", "onReady", 2, 3, "ready", 4, 5)).toBe(true);
  rootA.host.invalidateAuthoritativeGeneration(7, generationA);
  expect(registerLingoCallbackForOwner(rootA.host, 7, generationA, "_root", "onReady", 2, 3, "ready", 4, 5)).toBe(false);
  rootA.dispose();
  rootB.dispose();

  const bridge = registerFlashOwner("session-lingo:bridge", fakeFlashCapability("session-lingo:bridge"));
  const bridgeGeneration = bridge.host.reserveInstanceGeneration(7);
  publishFlashInstanceForOwner(bridge.host, {
    host: bridge.host,
    browserHandle: bridge.host.capability,
    spriteNum: 7,
    instanceGeneration: bridgeGeneration,
    castLib: 1,
    castMember: 1,
    rufflePlayer: { remove: () => {} },
    bridgeId: "missing-reply",
    container: { remove: () => {} } as any,
    canvas: null,
    width: 1,
    height: 1,
    nativeW: 1,
    nativeH: 1,
    animFrameId: null,
    ready: true,
  } as any);
  expect(registerLingoCallbackForOwner(bridge.host, 7, bridgeGeneration, "_root", "onReady", 2, 3, "ready", 4, 5)).toBe(false);
  bridge.dispose();
});

test("stable Ruffle callback route validates generation, preserves wire payload, and isolates disposal", () => {
  const capA = fakeFlashCapability("session-route:a") as any;
  const capB = fakeFlashCapability("session-route:b") as any;
  const receivedA: string[] = [];
  const receivedB: string[] = [];
  capA.trigger_lingo_callback_on_script_ruffle = (...args: unknown[]) => {
    receivedA.push(args[6] as string);
    return true;
  };
  capB.trigger_lingo_callback_on_script_ruffle = (...args: unknown[]) => {
    receivedB.push(args[6] as string);
    return true;
  };
  const rootA = registerFlashOwner("session-route:a", capA);
  const rootB = registerFlashOwner("session-route:b", capB);
  const generationA = rootA.host.reserveInstanceGeneration(7);
  const generationB = rootB.host.reserveInstanceGeneration(7);
  const instance = (host: typeof rootA.host, generation: number) => ({
    host,
    browserHandle: host.capability,
    spriteNum: 7,
    instanceGeneration: generation,
    castLib: 1,
    castMember: 1,
    rufflePlayer: { remove: () => {} },
    bridgeId: null,
    container: { remove: () => {} } as any,
    canvas: null,
    width: 1,
    height: 1,
    nativeW: 1,
    nativeH: 1,
    animFrameId: null,
    ready: true,
  });
  publishFlashInstanceForOwner(rootA.host, instance(rootA.host, generationA) as any);
  publishFlashInstanceForOwner(rootB.host, instance(rootB.host, generationB) as any);

  const malformed = "not-json";
  expect(deliverOwnedLingoCallback("session-route:a", 7, generationA - 1, 2, 3, "onReady", malformed, 4, 5)).toBe(false);
  expect(receivedA).toEqual([]);
  expect(deliverOwnedLingoCallback("session-route:a", 7, generationA, 2, 3, "onReady", malformed, 4, 5)).toBe(true);
  expect(receivedA).toEqual([malformed]);
  expect(deliverOwnedLingoCallback("session-route:b", 7, generationB, 2, 3, "onReady", "[\"encoded\"]", 4, 5)).toBe(true);
  expect(receivedB).toEqual(["[\"encoded\"]"]);

  rootA.dispose();
  expect(deliverOwnedLingoCallback("session-route:a", 7, generationA, 2, 3, "onReady", "[]", 4, 5)).toBe(false);
  expect(deliverOwnedLingoCallback("session-route:b", 7, generationB, 2, 3, "onReady", "[]", 4, 5)).toBe(true);
  expect(receivedB).toEqual(["[\"encoded\"]", "[]"]);
  rootB.dispose();
});

test("owner callback registration rolls back an exact map entry when prepared flush throws", () => {
  const ownerKey = "session-callback-rollback:root";
  onFlashMemberLoadedPrepared(1, 1, 1, new Uint8Array([70, 87, 83]), 1, 1, true, 1, ownerKey, 1);
  expect(() => registerVmCallbacks({
    onFlashMemberLoaded: () => { throw new Error("injected prepared flush failure"); },
  } as any, ownerKey, false)).toThrow("injected prepared flush failure");
  expect(dispatchVmCallback(ownerKey, "onFlashMemberLoaded")).toBeUndefined();

  const dispose = registerVmCallbacks({
    onFlashMemberLoaded: () => "replacement",
  } as any, ownerKey, false);
  expect(dispatchVmCallback(ownerKey, "onFlashMemberLoaded")).toBe("replacement");
  dispose();
});

test("capability failure retires the controller subtree and preserves replacement identity", () => {
  const rootKey = "session-failure:root";
  const childKey = "session-failure:child";
  const failingCapability = fakeFlashCapability(rootKey);
  failingCapability.set_flash_scripted_access_pending = () => {
    throw new Error("root capability retired");
  };
  const root = registerFlashOwner(rootKey, failingCapability);
  const callbackEntries = new Map<string, { callbacks: any; dispose: () => void }>();
  const controller = createFlashOwnerController((callbacks, ownerKey, setAsDefault) => {
    const disposeRegistered = registerVmCallbacks(callbacks as any, ownerKey, setAsDefault);
    const dispose = () => {
      if (callbackEntries.get(ownerKey)?.callbacks === callbacks) callbackEntries.delete(ownerKey);
      disposeRegistered();
    };
    callbackEntries.set(ownerKey, { callbacks, dispose });
    return dispose;
  });

  controller.registerNested(rootKey, childKey, fakeFlashCapability(childKey));
  const staleRegistration = callbackEntries.get(childKey);
  expect(staleRegistration).toBeDefined();
  root.host.beginScriptedAccess(1);
  expect(root.host.disposed).toBe(true);
  expect(callbackEntries.has(childKey)).toBe(false);

  const replacementRoot = registerFlashOwner(rootKey, fakeFlashCapability(rootKey));
  controller.registerNested(rootKey, childKey, fakeFlashCapability(childKey));
  expect(callbackEntries.has(childKey)).toBe(true);
  staleRegistration?.dispose();
  expect(callbackEntries.has(childKey)).toBe(true);
  expect(dispatchVmCallback(childKey, "onFlashGetVariable", 1, "_root.value")).toBeNull();

  controller.retireNested(rootKey, childKey);
  replacementRoot.dispose();
});

test("child capability failure retires its exact subtree while siblings and other roots survive", () => {
  const rootAKey = "session-child-failure:a";
  const rootBKey = "session-child-failure:b";
  const childKey = "session-child-failure:a:child";
  const grandchildKey = "session-child-failure:a:grandchild";
  const siblingKey = "session-child-failure:a:sibling";
  const independentKey = "session-child-failure:b:child";
  const rootA = registerFlashOwner(rootAKey, fakeFlashCapability(rootAKey));
  const rootB = registerFlashOwner(rootBKey, fakeFlashCapability(rootBKey));
  const failingChildCapability = fakeFlashCapability(childKey);
  failingChildCapability.set_flash_scripted_access_pending = () => {
    throw new Error("child capability retired");
  };
  const callbackEntries = new Map<string, { callbacks: any; dispose: () => void }>();
  const controller = createFlashOwnerController((callbacks, ownerKey, setAsDefault) => {
    const disposeRegistered = registerVmCallbacks(callbacks as any, ownerKey, setAsDefault);
    const dispose = () => {
      if (callbackEntries.get(ownerKey)?.callbacks === callbacks) callbackEntries.delete(ownerKey);
      disposeRegistered();
    };
    callbackEntries.set(ownerKey, { callbacks, dispose });
    return dispose;
  });

  controller.registerNested(rootAKey, childKey, failingChildCapability);
  controller.registerNested(childKey, grandchildKey, fakeFlashCapability(grandchildKey));
  controller.registerNested(rootAKey, siblingKey, fakeFlashCapability(siblingKey));
  controller.registerNested(rootBKey, independentKey, fakeFlashCapability(independentKey));
  const staleChildRegistration = callbackEntries.get(childKey);
  expect(staleChildRegistration).toBeDefined();

  expect(dispatchVmCallback(childKey, "onFlashGetVariable", 1, "_root.value")).toBeNull();
  expect(callbackEntries.has(childKey)).toBe(false);
  expect(callbackEntries.has(grandchildKey)).toBe(false);
  expect(callbackEntries.has(siblingKey)).toBe(true);
  expect(callbackEntries.has(independentKey)).toBe(true);
  expect(rootA.host.disposed).toBe(false);
  expect(rootB.host.disposed).toBe(false);

  controller.registerNested(rootAKey, childKey, fakeFlashCapability(childKey));
  staleChildRegistration?.dispose();
  expect(callbackEntries.has(childKey)).toBe(true);
  controller.registerNested(childKey, grandchildKey, fakeFlashCapability(grandchildKey));

  controller.retireNested(rootAKey, siblingKey);
  controller.retireNested(rootAKey, childKey);
  controller.retireNested(rootBKey, independentKey);
  rootA.dispose();
  rootB.dispose();
});
