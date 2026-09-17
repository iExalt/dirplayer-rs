import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile, unlink, writeFile } from 'node:fs/promises';

// This harness imports the production FlashPlayerManager module.  The DOM and
// Ruffle bridge below are deliberately only transport scaffolding: owner
// registration, instance publication, and teardown all execute the real
// FlashOwnerHost/create/destroy code.

const listeners = new Map();
const players = new Map();
const playerOwners = new Map();
const capabilities = new Map();
const removedPlayers = [];
let nextPlayer = 1;
let ownerBeingCreated = null;
let holdLoadResponses = false;
const heldLoadResponses = [];

class FakeEvent {
  constructor(type, init = {}) {
    this.type = type;
    this.detail = init.detail;
  }
}

class FakeNode {
  constructor(tagName) {
    this.tagName = tagName;
    this.style = {};
    this.children = [];
    this.parentNode = null;
    this.shadowRoot = null;
    this.isConnected = false;
    this.width = 0;
    this.height = 0;
  }

  appendChild(child) {
    child.parentNode = this;
    child.isConnected = true;
    this.children.push(child);
    return child;
  }

  remove() {
    if (!this.isConnected && this.tagName !== 'ruffle-player') return;
    this.isConnected = false;
    if (this.parentNode) {
      this.parentNode.children = this.parentNode.children.filter((x) => x !== this);
      this.parentNode = null;
    }
    if (this.tagName === 'ruffle-player' && this.onRemove) this.onRemove();
  }

  querySelector() {
    return null;
  }
}

class FakeCanvasElement extends FakeNode {
  getContext() {
    return null;
  }
}

const body = new FakeNode('body');
body.isConnected = true;

globalThis.CustomEvent = FakeEvent;
globalThis.HTMLCanvasElement = FakeCanvasElement;
globalThis.window = {
  customElements: null,
  location: { origin: 'http://flash-owner.test' },
  __dirplayerFlashConfig: {},
  addEventListener(type, callback) {
    const set = listeners.get(type) ?? new Set();
    set.add(callback);
    listeners.set(type, set);
  },
  removeEventListener(type, callback) {
    listeners.get(type)?.delete(callback);
  },
  dispatchEvent(event) {
    for (const callback of listeners.get(event.type) ?? []) callback(event);
    return true;
  },
};

globalThis.document = {
  body,
  documentElement: body,
  createElement(tagName) {
    return tagName === 'canvas' ? new FakeCanvasElement(tagName) : new FakeNode(tagName);
  },
  querySelector(selector) {
    const match = /data-dirplayer-bridge-id="([^"]+)"/.exec(selector);
    return match ? players.get(match[1]) ?? null : null;
  },
  getElementById() {
    return null;
  },
};

globalThis.requestAnimationFrame = (callback) => setTimeout(() => callback(Date.now()), 0);
globalThis.cancelAnimationFrame = (id) => clearTimeout(id);
globalThis.performance = { now: () => Date.now() };

// The production manager waits 500ms and 3s around Ruffle readiness. The fake
// bridge is immediate, so collapse only those waits to keep this focused test
// bounded while retaining normal timer behavior for all other code.
const realSetTimeout = globalThis.setTimeout;
globalThis.setTimeout = (callback, delay, ...args) =>
  realSetTimeout(callback, delay === 500 || delay === 3000 ? 0 : delay, ...args);

window.addEventListener('dirplayer-ruffle-bridge-request', (event) => {
  const { requestId, method } = event.detail;
  let result = null;
  if (method === 'isReady') {
    result = true;
  } else if (method === 'createPlayer') {
    const playerId = `fake-player-${nextPlayer++}`;
    const player = new FakeNode('ruffle-player');
    player.isConnected = true;
    player.onRemove = () => {
      removedPlayers.push(playerId);
      const owner = playerOwners.get(playerId);
      capabilities.get(owner)?.trigger_lingo_callback_on_script(1, 1, 'teardownReentry', '[]', 1, 1);
    };
    players.set(playerId, player);
    playerOwners.set(playerId, ownerBeingCreated);
    result = { playerId };
  } else if (method === 'destroyPlayer') {
    result = null;
  } else if (method === 'callMethod' || method === 'registerCallbackForwarders') {
    if (method === 'callMethod' && event.detail.methodName === 'load' && holdLoadResponses) {
      heldLoadResponses.push(() => window.dispatchEvent(new CustomEvent('dirplayer-ruffle-bridge-response', {
        detail: { requestId, result: null },
      })));
      return;
    }
    result = null;
  }
  queueMicrotask(() => window.dispatchEvent(new CustomEvent('dirplayer-ruffle-bridge-response', {
    detail: { requestId, result },
  })));
});

const manager = await import('../src/services/flashPlayerManager.ts');
const vmApi = await import('../dirplayer-js-api/index.js');

function makeCapability(ownerKey, state) {
  let host;
  const capability = {
    owner_identity: () => ownerKey,
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => {
      state.reentryDisposed = host?.disposed ?? false;
      return false;
    },
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const registration = manager.registerFlashOwner(ownerKey, capability);
  host = registration.host;
  capabilities.set(ownerKey, capability);
  return { registration, host, state };
}

function swfFixture() {
  return new Uint8Array([0x46, 0x57, 0x53, 0x09]);
}

test('production initFlashBridge uses owner routes and per-instance callbacks', async () => {
  const calls = { localA: 0, localB: 0 };
  const makeBridgeCapability = (ownerKey, slot) => {
    let host;
    const state = {};
    const capability = {
      owner_identity: () => ownerKey,
      update_flash_frame: () => {},
      trigger_lingo_callback_on_script: () => {
        state.reentryDisposed = host?.disposed ?? false;
        return false;
      },
      local_connection_send: () => {
        calls[slot] += 1;
        return true;
      },
      dispatch_flash_event: () => false,
      dispatch_flash_lingo: async () => false,
    };
    const bridge = manager.initFlashBridge(capability);
    host = bridge.host;
    capabilities.set(ownerKey, capability);
    const disposeCallbacks = vmApi.registerVmCallbacks({
      onFlashPlayOwned: (spriteNum) => manager.playFlashForOwner(host, spriteNum),
      onFlashLocalConnectionSendOwned: (name, method, argsJson) =>
        manager.localConnectionSendForOwner(host, name, method, argsJson),
    }, ownerKey);
    return { bridge, host, state, disposeCallbacks };
  };

  const a = makeBridgeCapability('bridge-owner-a:g1', 'localA');
  const b = makeBridgeCapability('bridge-owner-b:g1', 'localB');

  assert.equal(window.dirplayer_localConnectionSendOwned('bridge-owner-a:g1', 'c', 'm', '[]'), true);
  assert.equal(window.dirplayer_localConnectionSendOwned('bridge-owner-b:g1', 'c', 'm', '[]'), true);
  assert.deepEqual(calls, { localA: 1, localB: 1 });
  // The old unqualified bridge has no owner key and therefore retains its
  // legacy single-owner behavior (the most recently installed bridge).
  assert.equal(window.dirplayer_localConnectionSend('c', 'm', '[]'), true);
  assert.deepEqual(calls, { localA: 1, localB: 2 });

  ownerBeingCreated = 'bridge-owner-a:g1';
  await manager.createFlashInstanceForOwner(a.host, 7, 2, 3, swfFixture(), 32, 24);
  ownerBeingCreated = 'bridge-owner-b:g1';
  await manager.createFlashInstanceForOwner(b.host, 7, 2, 3, swfFixture(), 32, 24);
  assert.equal(a.host.instances.has('bridge-owner-a:g1:7'), true);
  assert.equal(b.host.instances.has('bridge-owner-b:g1:7'), true);

  // These calls use each instance's owner-bound player callback. They must not
  // be redirected by the later registration of B.
  window.dirplayer_rufflePlayOwned('bridge-owner-a:g1', 7);
  window.dirplayer_rufflePlayOwned('bridge-owner-b:g1', 7);

  b.bridge();
  b.disposeCallbacks();
  assert.equal(b.state.reentryDisposed, true);
  assert.equal(a.bridge.host.disposed, false);
  assert.equal(a.host.instances.has('bridge-owner-a:g1:7'), true);
  assert.equal(b.host.instances.size, 0);
  assert.equal(window.dirplayer_localConnectionSendOwned('bridge-owner-b:g1', 'c', 'm', '[]'), false);

  // Disposing B restores A's closure, so A remains callable without a global
  // owner registry or a lookup through the active player.
  window.dirplayer_rufflePlayOwned('bridge-owner-a:g1', 7);
  assert.equal(window.dirplayer_localConnectionSendOwned('bridge-owner-a:g1', 'c', 'm', '[]'), true);
  assert.deepEqual(calls, { localA: 2, localB: 2 });

  a.bridge();
  a.disposeCallbacks();
  assert.equal(a.state.reentryDisposed, true);
  assert.equal(window.dirplayer_localConnectionSendOwned?.('bridge-owner-a:g1', 'c', 'm', '[]') ?? false, false);
});

test('production owner routing drops a disposed owner on non-LIFO teardown', () => {
  const makeOwner = (ownerKey) => {
    let host;
    const capability = {
      owner_identity: () => ownerKey,
      update_flash_frame: () => {},
      trigger_lingo_callback_on_script: () => false,
      local_connection_send: () => true,
      dispatch_flash_event: () => false,
      dispatch_flash_lingo: async () => false,
    };
    const bridge = manager.initFlashBridge(capability);
    host = bridge.host;
    const disposeCallbacks = vmApi.registerVmCallbacks({
      onFlashPlayOwned: (spriteNum) => manager.playFlashForOwner(host, spriteNum),
      onFlashLocalConnectionSendOwned: (name, method, argsJson) =>
        manager.localConnectionSendForOwner(host, name, method, argsJson),
    }, ownerKey);
    return { bridge, disposeCallbacks };
  };

  const a = makeOwner('non-lifo-a:g1');
  const b = makeOwner('non-lifo-b:g1');
  a.bridge();
  a.disposeCallbacks();
  assert.equal(window.dirplayer_localConnectionSendOwned('non-lifo-a:g1', 'c', 'm', '[]'), false);
  assert.equal(window.dirplayer_localConnectionSendOwned('non-lifo-b:g1', 'c', 'm', '[]'), true);
  b.bridge();
  b.disposeCallbacks();
  assert.equal(window.dirplayer_localConnectionSendOwned('non-lifo-b:g1', 'c', 'm', '[]'), false);
});

test('production FlashOwnerHost isolates overlapping sprites and tears down reentrantly', async () => {
  const removedBefore = removedPlayers.length;
  const a = makeCapability('owner-a:g1', {});
  const b = makeCapability('owner-b:g1', {});

  ownerBeingCreated = 'owner-a:g1';
  await manager.createFlashInstanceForOwner(a.host, 7, 2, 3, swfFixture(), 32, 24);
  ownerBeingCreated = 'owner-b:g1';
  await manager.createFlashInstanceForOwner(b.host, 7, 2, 3, swfFixture(), 32, 24);

  assert.equal(a.host.instances.size, 1);
  assert.equal(b.host.instances.size, 1);

  a.registration.dispose();
  assert.equal(a.host.disposed, true);
  assert.equal(a.host.instances.size, 0);
  assert.equal(b.host.disposed, false);
  assert.equal(b.host.instances.size, 1);
  assert.equal(a.state.reentryDisposed, true);

  b.registration.dispose();
  assert.equal(b.host.disposed, true);
  assert.equal(b.host.instances.size, 0);
  assert.equal(removedPlayers.length - removedBefore, 2);
});

test('production registration rejects creation after disposal and clears pending owner work', async () => {
  const state = {};
  const owner = makeCapability('owner-cancel:g1', state);
  owner.host.pendingQueue.enqueueLegacy(4, { kind: 'play' });
  owner.host.pendingQueue.enqueueOwned('owner-cancel:g1', 4, { kind: 'stop' });
  owner.registration.dispose();
  assert.equal(owner.host.disposed, true);
  assert.deepEqual(owner.host.pendingQueue.drainReady(4, 'owner-cancel:g1:4', true, true), []);
  await assert.rejects(
    manager.createFlashInstanceForOwner(owner.host, 4, 1, 1, swfFixture(), 1, 1),
    /disposed/,
  );
  assert.equal(state.reentryDisposed ?? true, true);
});

test('browser bridge unregisters before a delayed manager import publishes an owner', async () => {
  const bundlePath = new URL('../vm-rust/tests/browser_templates/flashPlayerManager.bundle.js', import.meta.url);
  const realBridgePath = new URL('../vm-rust/tests/browser_templates/dirplayer-js-api-real.js', import.meta.url);
  let releaseBundle;
  window.__releaseFlashBundle = new Promise((resolve) => { releaseBundle = resolve; });
  await writeFile(bundlePath, `await window.__releaseFlashBundle;\nexport * from '../../../src/services/flashPlayerManager.ts';\n`);
  await writeFile(realBridgePath, await readFile(new URL('../dirplayer-js-api/index.js', import.meta.url)));
  try {
    const bridge = await import('../vm-rust/tests/browser_templates/dirplayer-js-api.js');
    const key = 'owner-import:g1';
    let identityCalls = 0;
    const capability = {
      owner_identity: () => { identityCalls += 1; return key; },
      update_flash_frame: () => {},
      trigger_lingo_callback_on_script: () => false,
      local_connection_send: () => false,
      dispatch_flash_event: () => false,
      dispatch_flash_lingo: async () => false,
    };

    bridge.dirplayer_registerFlashOwner(key, capability);
    bridge.dirplayer_unregisterFlashOwner(key);
    releaseBundle();
    await new Promise((resolve) => realSetTimeout(resolve, 0));
    await new Promise((resolve) => realSetTimeout(resolve, 0));

    assert.equal(identityCalls, 0, 'disposed import must not publish a FlashOwnerHost');
  } finally {
    releaseBundle?.();
    await unlink(bundlePath).catch(() => {});
    await unlink(realBridgePath).catch(() => {});
  }
});

test('browser bridge retires an in-flight production creation after owner disposal', async () => {
  const bridge = await import('../vm-rust/tests/browser_templates/dirplayer-js-api.js');
  const key = 'owner-flight:g1';
  let identityCalls = 0;
  let localCalls = 0;
  const capability = {
    owner_identity: () => { identityCalls += 1; return key; },
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => { localCalls += 1; return true; },
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };

  bridge.dirplayer_registerFlashOwner(key, capability);
  for (let i = 0; i < 20 && typeof window.dirplayer_localConnectionSend !== 'function'; i += 1) {
    await new Promise((resolve) => realSetTimeout(resolve, 0));
  }
  assert.equal(typeof window.dirplayer_localConnectionSend, 'function');
  assert.equal(window.dirplayer_localConnectionSend('conn', 'method', '[]'), true);
  assert.equal(localCalls, 1, 'bridge global routes through the registered owner capability');
  ownerBeingCreated = key;
  holdLoadResponses = true;
  bridge.onFlashMemberLoaded(8, 2, 3, swfFixture(), 32, 24, false, -1, key);

  for (let i = 0; i < 20 && heldLoadResponses.length === 0; i += 1) {
    await new Promise((resolve) => realSetTimeout(resolve, 0));
  }
  assert.equal(heldLoadResponses.length > 0, true, 'creation reached the controlled load wait');

  bridge.dirplayer_unregisterFlashOwner(key);
  holdLoadResponses = false;
  for (const release of heldLoadResponses.splice(0)) release();
  // The production create path retains its normal post-load ActionScript
  // initialization window even after the owner is retired; wait for that
  // owned future to reach its stale-generation cleanup branch.
  await new Promise((resolve) => realSetTimeout(resolve, 3500));

  assert.equal(
    window.dirplayer_flashInstances?.has(`${key}:8`) ?? false,
    false,
    'stale in-flight player was never published into the production instance map',
  );
  assert.equal(identityCalls >= 1, true);
});

test('owner-qualified bridge callbacks remain routed after a later owner registers', async () => {
  const bridge = await import('../vm-rust/tests/browser_templates/dirplayer-js-api.js');
  const calls = { a: 0, b: 0 };
  const makeCapability = (key, slot) => ({
    owner_identity: () => key,
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => { calls[slot] += 1; return true; },
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  });

  bridge.dirplayer_registerFlashOwner('owner-route-a:g1', makeCapability('owner-route-a:g1', 'a'));
  bridge.dirplayer_registerFlashOwner('owner-route-b:g1', makeCapability('owner-route-b:g1', 'b'));
  await new Promise((resolve) => realSetTimeout(resolve, 50));
  assert.equal(window.dirplayer_localConnectionSendOwned('owner-route-a:g1', 'c', 'm', '[]'), true);
  assert.equal(window.dirplayer_localConnectionSendOwned('owner-route-b:g1', 'c', 'm', '[]'), true);
  assert.deepEqual(calls, { a: 1, b: 1 });

  bridge.dirplayer_unregisterFlashOwner('owner-route-b:g1');
  assert.equal(window.dirplayer_localConnectionSendOwned('owner-route-a:g1', 'c', 'm', '[]'), true);
  assert.equal(window.dirplayer_localConnectionSendOwned('owner-route-b:g1', 'c', 'm', '[]'), false);
  assert.deepEqual(calls, { a: 2, b: 1 });
  bridge.dirplayer_unregisterFlashOwner('owner-route-a:g1');
});

test('direct Ruffle players bind distinct owners before their first load', async () => {
  const oldCustomElements = window.customElements;
  const oldRuffle = window.dirplayer_RufflePlayer;
  const loadOwners = [];
  window.customElements = {};
  window.dirplayer_RufflePlayer = {
    newest() {
      return {
        createPlayer() {
          const player = new FakeNode('ruffle-player');
          player.ruffle = () => ({ load: async () => { loadOwners.push(player.ownerKey); } });
          player.dirplayer_set_owner_key = (ownerKey) => {
            assert.equal(player.ownerKey ?? null, null);
            player.ownerKey = ownerKey;
          };
          player.dirplayer_addOpenUrlHandler = () => {};
          player.dirplayer_addFSCommandHandler = () => {};
          return player;
        },
      };
    },
  };

  const a = makeCapability('direct-owner-a:g1', {});
  const b = makeCapability('direct-owner-b:g1', {});
  try {
    // Direct creation reaches the real owner setter before `ruffle().load`.
    await manager.createFlashInstanceForOwner(a.host, 11, 2, 3, swfFixture(), 32, 24);
    await manager.createFlashInstanceForOwner(b.host, 11, 2, 3, swfFixture(), 32, 24);
    assert.deepEqual(loadOwners, ['direct-owner-a:g1', 'direct-owner-b:g1']);
  } finally {
    a.registration.dispose();
    b.registration.dispose();
    window.customElements = oldCustomElements;
    window.dirplayer_RufflePlayer = oldRuffle;
  }
});
