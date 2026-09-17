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
const playerCommands = new Map();
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
    if (method === 'callMethod' && event.detail.methodName !== 'load') {
      const commands = playerCommands.get(event.detail.playerId) ?? [];
      commands.push({ method: event.detail.methodName, args: event.detail.args });
      playerCommands.set(event.detail.playerId, commands);
    }
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

function fakeBindingAuthority(state = {}) {
  const bindingState = state.bindingState ?? { nextGeneration: 1, current: new Map() };
  state.bindingState = bindingState;
  return {
    reserve_flash_instance_generation(spriteNum) {
      if (bindingState.nextGeneration >= Number.MAX_SAFE_INTEGER) {
        throw new Error('Flash instance generation exhausted');
      }
      const generation = bindingState.nextGeneration++;
      bindingState.current.set(spriteNum, generation);
      return generation;
    },
    invalidate_flash_instance_generation(spriteNum, expectedGeneration) {
      if (bindingState.current.get(spriteNum) !== expectedGeneration) return false;
      bindingState.current.delete(spriteNum);
      return true;
    },
    is_flash_instance_generation_current(spriteNum, expectedGeneration) {
      return bindingState.current.get(spriteNum) === expectedGeneration;
    },
  };
}

function makeCapability(ownerKey, state) {
  let host;
  const bindingState = state.bindingState ?? { nextGeneration: 1, current: new Map() };
  state.bindingState = bindingState;
  const capability = {
    owner_identity: () => ownerKey,
    ...fakeBindingAuthority(state),
    set_flash_scripted_access_pending: (pending) => { state.scriptedAccessPending = pending; },
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

function commandsForOwner(ownerKey) {
  for (const [playerId, playerOwner] of playerOwners) {
    if (playerOwner === ownerKey) return playerCommands.get(playerId) ?? [];
  }
  return [];
}

function installReadyOwnerInstance(host, spriteNum, state) {
  const instanceGeneration = host.reserveInstanceGeneration(spriteNum);
  const player = {
    isPlaying: true,
    GetVariable(path) {
      state.gets.push(path);
      return path === '/:_totalframes' ? '12' : path === '/:_currentframe' ? '4' : `value:${path}`;
    },
    SetVariable(path, value) { state.sets.push([path, value]); return true; },
    CallFunction(path, args) { state.calls.push([path, args]); return `${host.ownerKey}:${path}`; },
    GotoFrame(frame, stop) { state.gotos.push([frame, stop]); },
    dirplayer_hitTest(x, y) { state.hitTests.push([x, y]); return 2; },
  };
  const instance = {
    host,
    browserHandle: host.capability,
    spriteNum,
    instanceGeneration,
    castLib: 0,
    castMember: 0,
    rufflePlayer: player,
    bridgeId: null,
    container: { remove() {} },
    canvas: null,
    width: 1,
    height: 1,
    nativeW: 1,
    nativeH: 1,
    animFrameId: null,
    ready: true,
    pausedAtStart: false,
    stopped: false,
  };
  host.instances.set(`${host.ownerKey}:${spriteNum}`, instance);
  window.dirplayer_flashInstances?.set(`${host.ownerKey}:${spriteNum}`, instance);
  return player;
}

test('production initFlashBridge uses owner routes and per-instance callbacks', async () => {
  const calls = { localA: 0, localB: 0 };
  const makeBridgeCapability = (ownerKey, slot) => {
    let host;
    const state = {};
    const capability = {
      owner_identity: () => ownerKey,
      ...fakeBindingAuthority(),
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

  // Disposing B removes only B's generation, so the exact owner route for A
  // remains callable without consulting the ambiguous sprite index.
  window.dirplayer_rufflePlayOwned('bridge-owner-a:g1', 7);
  assert.equal(window.dirplayer_localConnectionSendOwned('bridge-owner-a:g1', 'c', 'm', '[]'), true);
  assert.deepEqual(calls, { localA: 2, localB: 2 });

  a.bridge();
  a.disposeCallbacks();
  assert.equal(a.state.reentryDisposed, true);
  assert.equal(window.dirplayer_localConnectionSendOwned?.('bridge-owner-a:g1', 'c', 'm', '[]') ?? false, false);
});

test('initFlashBridge routes every owner operation by exact owner generation', () => {
  const makeOwner = (ownerKey) => {
    const state = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
    const capability = {
      owner_identity: () => ownerKey,
      ...fakeBindingAuthority(),
      set_flash_scripted_access_pending: () => {},
      update_flash_frame: () => {},
      trigger_lingo_callback_on_script: () => false,
      local_connection_send: () => false,
      dispatch_flash_event: () => false,
      dispatch_flash_lingo: async () => false,
    };
    const bridge = manager.initFlashBridge(capability);
    installReadyOwnerInstance(bridge.host, 7, state);
    return { bridge, state };
  };

  const a = makeOwner('operation-owner-a:g1');
  const b = makeOwner('operation-owner-b:g1');

  assert.equal(window.dirplayer_ruffleGetVariableOwned('operation-owner-a:g1', 7, '/:_x'), 'value:/:_x');
  assert.equal(window.dirplayer_ruffleSetVariableOwned('operation-owner-b:g1', 7, '/:_x', '9'), true);
  assert.equal(window.dirplayer_ruffleCallFunctionOwned('operation-owner-a:g1', 7, '_root.boot', '[]'), 'operation-owner-a:g1:_root.boot');
  assert.equal(window.dirplayer_ruffleIsPlayingOwned('operation-owner-b:g1', 7), true);
  window.dirplayer_ruffleGoToFrameOwned('operation-owner-b:g1', 7, 'warm0');
  window.dirplayer_ruffleStopOwned('operation-owner-a:g1', 7);
  window.dirplayer_ruffleRewindOwned('operation-owner-b:g1', 7);
  window.dirplayer_ruffleCallFrameOwned('operation-owner-a:g1', 7, 6);
  assert.equal(window.dirplayer_ruffleGetFrameCountOwned('operation-owner-a:g1', 7), 12);
  assert.equal(window.dirplayer_ruffleGetCurrentFrameOwned('operation-owner-b:g1', 7), 4);
  assert.equal(window.dirplayer_ruffleHitTestOwned('operation-owner-a:g1', 7, 3, 4), 2);
  assert.equal(window.dirplayer_ruffleGetFlashPropertyOwned('operation-owner-b:g1', 7, '', 0), 'value:/:_x');
  assert.equal(window.dirplayer_ruffleSetFlashPropertyOwned('operation-owner-a:g1', 7, '', 0, '11'), true);
  const operationGeneration = b.bridge.host.instanceGenerations.get(7);
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned('operation-owner-b:g1', 7, operationGeneration),
    { ok: true, generation: operationGeneration, ready: true },
  );

  // The two same-number instances were reached through their own host state.
  assert.deepEqual(a.state.gets, ['/:_x', '/:_totalframes']);
  assert.deepEqual(b.state.gets, ['/:_currentframe', '/:_x']);
  assert.deepEqual(a.state.calls, [['_root.boot', []], ['_root.stop', []]]);
  assert.deepEqual(b.state.sets, [['/:_x', '9']]);
  assert.deepEqual(a.state.gotos, [[6, true]]);
  assert.deepEqual(b.state.gotos, [[1, true]]);
  assert.deepEqual(b.state.calls, [['_root.gotoAndPlay', ['warm0']]]);
  assert.deepEqual(a.state.hitTests, [[3, 4]]);

  // Unknown and retired generations fail closed without touching either host.
  const beforeA = JSON.stringify(a.state);
  const beforeB = JSON.stringify(b.state);
  assert.equal(window.dirplayer_ruffleGetVariableOwned('operation-owner-unknown:g1', 7, '/:_x'), null);
  assert.equal(window.dirplayer_ruffleSetVariableOwned('operation-owner-unknown:g1', 7, '/:_x', 'x'), false);
  assert.equal(window.dirplayer_ruffleIsPlayingOwned('operation-owner-unknown:g1', 7), false);
  assert.equal(JSON.stringify(a.state), beforeA);
  assert.equal(JSON.stringify(b.state), beforeB);

  // Non-LIFO disposal removes only A; B remains routable despite the shared
  // sprite number, while the retired A generation cannot fall through.
  a.bridge();
  assert.equal(window.dirplayer_ruffleSetVariableOwned('operation-owner-a:g1', 7, '/:_x', 'retired'), false);
  assert.equal(window.dirplayer_ruffleSetVariableOwned('operation-owner-b:g1', 7, '/:_x', 'still-live'), true);
  assert.deepEqual(b.state.sets, [['/:_x', '9'], ['/:_x', 'still-live']]);
  b.bridge();
});

test('generation-aware Flash routes return typed results and reject stale generations', () => {
  const makeOwner = (ownerKey) => {
    const state = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
    const capability = {
      owner_identity: () => ownerKey,
      ...fakeBindingAuthority(),
      set_flash_scripted_access_pending: () => {},
      update_flash_frame: () => {},
      trigger_lingo_callback_on_script: () => false,
      local_connection_send: () => false,
      dispatch_flash_event: () => false,
      dispatch_flash_lingo: async () => false,
    };
    const bridge = manager.initFlashBridge(capability);
    installReadyOwnerInstance(bridge.host, 7, state);
    return { bridge, host: bridge.host, state };
  };

  const a = makeOwner('generation-route-a:g1');
  const b = makeOwner('generation-route-b:g1');
  const generationA = a.host.instanceGenerations.get(7);
  const generationB = b.host.instanceGenerations.get(7);
  assert.equal(generationA, 1);
  assert.equal(generationB, 1);

  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedForBinding('generation-route-a:g1', 7, '/:_x', false),
    { ok: true, generation: 1, value: 'value:/:_x' },
  );
  const objectValue = { identity: 'generation-route-a-object' };
  const objectInstance = a.host.instances.get('generation-route-a:g1:7');
  assert.ok(objectInstance);
  objectInstance.rufflePlayer.GetVariable = path => path === '/:object' ? objectValue : `value:${path}`;
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedAtGeneration('generation-route-a:g1', 7, 1, '/:object', true),
    { ok: true, generation: 1, value: objectValue },
  );
  assert.deepEqual(
    window.dirplayer_ruffleSetVariableOwnedAtGeneration('generation-route-b:g1', 7, 1, '/:_x', '9'),
    { ok: true, generation: 1, value: true },
  );
  assert.deepEqual(
    window.dirplayer_ruffleCallFunctionOwnedAtGeneration('generation-route-a:g1', 7, 1, '_root.boot', '[]'),
    { ok: true, generation: 1, value: 'generation-route-a:g1:_root.boot' },
  );
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedAtGeneration('generation-route-a:g1', 7, 99, '/:_x', false),
    { ok: false, code: 'stale-generation' },
  );
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedAtGeneration('generation-route-a:g1', 7, 0, '/:_x', false),
    { ok: false, code: 'invalid-generation' },
  );
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedAtGeneration('generation-route-missing:g1', 7, 1, '/:_x', false),
    { ok: false, code: 'unknown-owner' },
  );
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned('generation-route-a:g1', 7, 1),
    { ok: true, generation: 1, ready: true },
  );
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned('generation-route-b:g1', 7, 1),
    { ok: true, generation: 1, ready: true },
  );
  objectInstance.ready = false;
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned('generation-route-a:g1', 7, 1),
    { ok: true, generation: 1, ready: false },
  );
  objectInstance.ready = true;
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned('generation-route-a:g1', 7, 99),
    { ok: false, code: 'stale-generation' },
  );
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned('generation-route-missing:g1', 7, 1),
    { ok: false, code: 'unknown-owner' },
  );
  assert.equal(a.state.gets.includes('/:_x'), true);
  assert.deepEqual(b.state.sets, [['/:_x', '9']]);
  a.bridge();
  b.bridge();
  void generationA;
  void generationB;
});

test('readiness route fences replacement during ready observation and error formatting', () => {
  const makeOwner = (ownerKey) => {
    const state = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
    const capability = {
      owner_identity: () => ownerKey,
      ...fakeBindingAuthority(),
      set_flash_scripted_access_pending: () => {},
      update_flash_frame: () => {},
      trigger_lingo_callback_on_script: () => false,
      local_connection_send: () => false,
      dispatch_flash_event: () => false,
      dispatch_flash_lingo: async () => false,
    };
    const bridge = manager.initFlashBridge(capability);
    installReadyOwnerInstance(bridge.host, 7, state);
    return { bridge, host: bridge.host, state };
  };

  const replaceInstance = (host, key, replacementState, generation) => {
    host.invalidateInstanceGeneration(7, generation);
    host.instances.delete(key);
    window.dirplayer_flashInstances?.delete(key);
    installReadyOwnerInstance(host, 7, replacementState);
  };

  const observed = makeOwner('ready-observation:g1');
  const observedKey = 'ready-observation:g1:7';
  const observedInstance = observed.host.instances.get(observedKey);
  const replacementState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  assert.ok(observedInstance);
  Object.defineProperty(observedInstance, 'ready', {
    configurable: true,
    get() {
      replaceInstance(observed.host, observedKey, replacementState, 1);
      return true;
    },
  });
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned('ready-observation:g1', 7, 1),
    { ok: false, code: 'stale-generation' },
  );
  assert.deepEqual(observed.state.gets, []);
  assert.deepEqual(observed.state.calls, []);
  assert.equal(observed.host.scriptedAccessRequestCount(), 0);
  observed.bridge();

  const formatted = makeOwner('ready-error-format:g1');
  const formattedKey = 'ready-error-format:g1:7';
  const formattedInstance = formatted.host.instances.get(formattedKey);
  const formattedReplacementState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  assert.ok(formattedInstance);
  Object.defineProperty(formattedInstance, 'ready', {
    configurable: true,
    get() {
      const error = new Error();
      Object.defineProperty(error, 'message', {
        configurable: true,
        get() {
          replaceInstance(formatted.host, formattedKey, formattedReplacementState, 1);
          return 'readiness getter failed';
        },
      });
      throw error;
    },
  });
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned('ready-error-format:g1', 7, 1),
    { ok: false, code: 'stale-generation' },
  );
  assert.deepEqual(formatted.state.gets, []);
  assert.deepEqual(formatted.state.calls, []);
  assert.equal(formatted.host.scriptedAccessRequestCount(), 0);
  formatted.bridge();
});

test('generation-aware Flash route fences a reentrant same-sprite replacement', () => {
  const ownerKey = 'generation-reentry:g1';
  const oldState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const replacementState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const capability = {
    owner_identity: () => ownerKey,
    ...fakeBindingAuthority(),
    set_flash_scripted_access_pending: () => {},
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const bridge = manager.initFlashBridge(capability);
  const oldPlayer = installReadyOwnerInstance(bridge.host, 7, oldState);
  oldPlayer.CallFunction = () => {
    manager.destroyFlashInstance(bridge.host, 7);
    installReadyOwnerInstance(bridge.host, 7, replacementState);
    return 'stale-result';
  };

  assert.deepEqual(
    window.dirplayer_ruffleCallFunctionOwnedAtGeneration(ownerKey, 7, 1, '_root.reenter', '[]'),
    { ok: false, code: 'stale-generation' },
  );
  assert.deepEqual(replacementState.calls, []);
  bridge();
});

test('generation-aware Flash route rejects a reserved replacement before invoking the old instance', () => {
  const ownerKey = 'generation-pending-replacement:g1';
  const state = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const capability = {
    owner_identity: () => ownerKey,
    ...fakeBindingAuthority(),
    set_flash_scripted_access_pending: () => {},
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const bridge = manager.initFlashBridge(capability);
  const player = installReadyOwnerInstance(bridge.host, 7, state);
  player.GetVariable = () => {
    state.gets.push('unexpected');
    return 'unexpected';
  };
  bridge.host.instances.get(`${ownerKey}:7`).ready = false;
  bridge.host.reserveInstanceGeneration(7);
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedAtGeneration(ownerKey, 7, 1, '/:_x'),
    { ok: false, code: 'stale-generation' },
  );
  assert.deepEqual(state.gets, []);
  bridge();
});

test('not-ready generation route reports pending without queueing or host side effects', () => {
  const ownerKey = 'generation-not-ready-disposal:g1';
  let pendingNotifications = 0;
  const capability = {
    owner_identity: () => ownerKey,
    ...fakeBindingAuthority(),
    set_flash_scripted_access_pending: () => { pendingNotifications += 1; },
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const bridge = manager.initFlashBridge(capability);
  const state = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  installReadyOwnerInstance(bridge.host, 7, state);
  bridge.host.instances.get(`${ownerKey}:7`).ready = false;
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedForBinding(ownerKey, 7, '/:_x'),
    { ok: false, code: 'not-ready', generation: 1 },
  );
  assert.deepEqual(state.gets, []);
  assert.equal(pendingNotifications, 0);
  assert.deepEqual(bridge.host.pendingQueue.drainReady(7, `${ownerKey}:7`, true, true), []);
  bridge();
});

test('owned routes preserve a reserved generation before instance publication', () => {
  const ownerKey = 'generation-unpublished:g1';
  const state = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const owner = makeCapability(ownerKey, state);
  const generation = owner.host.reserveInstanceGeneration(7);

  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedForBinding(ownerKey, 7, '/:_x', false),
    { ok: false, code: 'not-ready', generation },
  );
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedAtGeneration(ownerKey, 7, generation, '/:_x', false),
    { ok: false, code: 'not-ready', generation },
  );
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned(ownerKey, 7, generation),
    { ok: true, generation, ready: false },
  );
  assert.deepEqual(state.gets, []);
  assert.deepEqual(owner.host.pendingQueue.drainReady(7, `${ownerKey}:7`, true, true), []);

  owner.host.invalidateInstanceGeneration(7, generation);
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedForBinding(ownerKey, 7, '/:_x', false),
    { ok: false, code: 'missing-instance' },
  );
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned(ownerKey, 7, generation),
    { ok: false, code: 'missing-instance' },
  );

  const replacementGeneration = owner.host.reserveInstanceGeneration(7);
  assert.equal(replacementGeneration > generation, true);
  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedAtGeneration(ownerKey, 7, generation, '/:_x', false),
    { ok: false, code: 'stale-generation' },
  );
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned(ownerKey, 7, replacementGeneration),
    { ok: true, generation: replacementGeneration, ready: false },
  );
  owner.registration.dispose();
});

test('reserved route revalidates a reentrant authority invalidation before pending result', () => {
  const ownerKey = 'generation-reentrant-reservation:g1';
  const state = {};
  const authority = fakeBindingAuthority(state);
  let host;
  let invalidateOnCheck = true;
  const capability = {
    owner_identity: () => ownerKey,
    ...authority,
    is_flash_instance_generation_current(spriteNum, expectedGeneration) {
      const current = authority.is_flash_instance_generation_current(spriteNum, expectedGeneration);
      if (invalidateOnCheck) {
        invalidateOnCheck = false;
        host.invalidateInstanceGeneration(spriteNum, expectedGeneration);
      }
      return current;
    },
    set_flash_scripted_access_pending: () => {},
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const registration = manager.registerFlashOwner(ownerKey, capability);
  host = registration.host;
  const generation = host.reserveInstanceGeneration(7);

  assert.deepEqual(
    window.dirplayer_ruffleGetVariableOwnedForBinding(ownerKey, 7, '/:_x', false),
    { ok: false, code: 'stale-generation' },
  );
  assert.equal(host.instanceGenerations.has(7), false);

  const replacementGeneration = host.reserveInstanceGeneration(7);
  invalidateOnCheck = true;
  assert.deepEqual(
    window.dirplayer_isFlashInstanceReadyOwned(ownerKey, 7, replacementGeneration),
    { ok: false, code: 'stale-generation' },
  );
  assert.equal(host.instanceGenerations.has(7), false);
  registration.dispose();
});

test('generation-aware Flash route fences replacement of the registered host object', () => {
  const ownerKey = 'generation-host-replacement:g1';
  const oldState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const replacementState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const capability = {
    owner_identity: () => ownerKey,
    ...fakeBindingAuthority(),
    set_flash_scripted_access_pending: () => {},
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const oldBridge = manager.initFlashBridge(capability);
  const oldPlayer = installReadyOwnerInstance(oldBridge.host, 7, oldState);
  let replacementBridge;
  oldPlayer.CallFunction = () => {
    oldBridge();
    replacementBridge = manager.initFlashBridge(capability);
    installReadyOwnerInstance(replacementBridge.host, 7, replacementState);
    return 'old-host-result';
  };
  assert.deepEqual(
    window.dirplayer_ruffleCallFunctionOwnedAtGeneration(ownerKey, 7, 1, '_root.reenter', '[]'),
    { ok: false, code: 'stale-generation' },
  );
  assert.ok(replacementBridge.host.instances.get(`${ownerKey}:7`));
  replacementBridge();
});

test('generation-aware Flash route fences a thenable getter that replaces the instance', () => {
  const ownerKey = 'generation-then-getter:g1';
  const oldState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const replacementState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const capability = {
    owner_identity: () => ownerKey,
    ...fakeBindingAuthority(),
    set_flash_scripted_access_pending: () => {},
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const bridge = manager.initFlashBridge(capability);
  const oldPlayer = installReadyOwnerInstance(bridge.host, 7, oldState);
  oldPlayer.CallFunction = () => ({
    get then() {
      manager.destroyFlashInstance(bridge.host, 7);
      installReadyOwnerInstance(bridge.host, 7, replacementState);
      return undefined;
    },
  });
  assert.deepEqual(
    window.dirplayer_ruffleCallFunctionOwnedAtGeneration(ownerKey, 7, 1, '_root.thenGetter', '[]'),
    { ok: false, code: 'stale-generation' },
  );
  assert.deepEqual(replacementState.calls, []);
  bridge();
});

test('generation-aware JS API wrappers do not fall back to legacy callbacks', () => {
  const previousLegacy = window.dirplayer_ruffleGetVariable;
  const previousGenerationRoute = window.dirplayer_ruffleGetVariableOwnedAtGeneration;
  let fallbackCalls = 0;
  window.dirplayer_ruffleGetVariable = () => {
    fallbackCalls += 1;
    return 'legacy-fallback';
  };
  delete window.dirplayer_ruffleGetVariableOwnedAtGeneration;
  try {
    assert.deepEqual(
      vmApi.dirplayer_ruffleGetVariableOwnedAtGeneration('missing-owner:g1', 7, 1, '/:_x'),
      { ok: false, code: 'unknown-owner' },
    );
    assert.equal(fallbackCalls, 0);
  } finally {
    window.dirplayer_ruffleGetVariable = previousLegacy;
    window.dirplayer_ruffleGetVariableOwnedAtGeneration = previousGenerationRoute;
  }
});

test('generation-aware JS API wrappers preserve get mode and readiness arguments', () => {
  const previousGetRoute = window.dirplayer_ruffleGetVariableOwnedAtGeneration;
  const previousBindRoute = window.dirplayer_ruffleGetVariableOwnedForBinding;
  const previousReadyRoute = window.dirplayer_isFlashInstanceReadyOwned;
  const seen = [];
  window.dirplayer_ruffleGetVariableOwnedAtGeneration = (...args) => {
    seen.push(['get', ...args]);
    return { ok: true, generation: 4, value: {} };
  };
  window.dirplayer_ruffleGetVariableOwnedForBinding = (...args) => {
    seen.push(['bind', ...args]);
    return { ok: true, generation: 4, value: 'scalar' };
  };
  window.dirplayer_isFlashInstanceReadyOwned = (...args) => {
    seen.push(['ready', ...args]);
    return { ok: true, generation: 4, ready: false };
  };
  try {
    vmApi.dirplayer_ruffleGetVariableOwnedForBinding('wrapper-owner:g1', 7, '/:obj', true);
    vmApi.dirplayer_ruffleGetVariableOwnedAtGeneration('wrapper-owner:g1', 7, 4, '/:obj', false);
    vmApi.dirplayer_isFlashInstanceReadyOwned('wrapper-owner:g1', 7, 4);
    assert.deepEqual(seen, [
      ['bind', 'wrapper-owner:g1', 7, '/:obj', true],
      ['get', 'wrapper-owner:g1', 7, 4, '/:obj', false],
      ['ready', 'wrapper-owner:g1', 7, 4],
    ]);
  } finally {
    window.dirplayer_ruffleGetVariableOwnedAtGeneration = previousGetRoute;
    window.dirplayer_ruffleGetVariableOwnedForBinding = previousBindRoute;
    window.dirplayer_isFlashInstanceReadyOwned = previousReadyRoute;
  }
});

test('owner seek pins are isolated from same-number stop and rewind operations', async () => {
  const makeOwner = (ownerKey) => {
    const state = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
    const capability = {
      owner_identity: () => ownerKey,
      ...fakeBindingAuthority(),
      set_flash_scripted_access_pending: () => {},
      update_flash_frame: () => {},
      trigger_lingo_callback_on_script: () => false,
      local_connection_send: () => false,
      dispatch_flash_event: () => false,
      dispatch_flash_lingo: async () => false,
    };
    const bridge = manager.initFlashBridge(capability);
    const player = installReadyOwnerInstance(bridge.host, 7, state);
    bridge.host.instances.get(`${ownerKey}:7`).pausedAtStart = true;
    return { bridge, host: bridge.host, state, player };
  };

  const a = makeOwner('pin-owner-a:g1');
  const b = makeOwner('pin-owner-b:g1');
  manager.goToFrameAndStopForOwner(a.host, 7, '5');
  // B has the same sprite number, but its stop must only clear B's pin map.
  window.dirplayer_ruffleStopOwned('pin-owner-b:g1', 7);
  window.dirplayer_ruffleRewindOwned('pin-owner-b:g1', 7);
  window.dirplayer_rufflePlayOwned('pin-owner-b:g1', 7);
  await Promise.resolve();
  assert.deepEqual(a.state.gotos, [[5, false], [5, true]]);
  assert.deepEqual(b.state.gotos, [[1, true], [4, false]]);
  assert.deepEqual(b.state.calls, [['_root.stop', []]]);
  a.bridge();
  b.bridge();
});

test('deferred seek cannot act on a replaced same-owner instance', async () => {
  const stateA = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const ownerKey = 'pin-replace-owner:g1';
  const capability = {
    owner_identity: () => ownerKey,
    ...fakeBindingAuthority(),
    set_flash_scripted_access_pending: () => {},
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const bridge = manager.initFlashBridge(capability);
  const oldPlayer = installReadyOwnerInstance(bridge.host, 7, stateA);
  bridge.host.instances.get(`${ownerKey}:7`).pausedAtStart = true;
  manager.goToFrameAndStopForOwner(bridge.host, 7, '6');

  const stateB = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const replacement = {
    ...bridge.host.instances.get(`${ownerKey}:7`),
    rufflePlayer: installReadyOwnerInstance(bridge.host, 7, stateB),
    instanceGeneration: bridge.host.instanceGenerations.get(7),
  };
  // installReadyOwnerInstance has installed the replacement in the exact
  // owner slot; keep the explicit object reference used by the old callback
  // only in oldPlayer/stateA.
  bridge.host.instances.set(`${ownerKey}:7`, replacement);
  window.dirplayer_flashInstances?.set(`${ownerKey}:7`, replacement);
  await Promise.resolve();
  assert.deepEqual(stateA.gotos, [[6, false]]);
  assert.deepEqual(stateB.gotos, []);
  bridge();
});

test('duplicate active owner registration is rejected and original remains usable', () => {
  const original = makeCapability('duplicate-owner:g1', {});
  assert.throws(
    () => manager.registerFlashOwner('duplicate-owner:g1', original.host.capability),
    /already registered/,
  );
  const ticket = original.host.beginScriptedAccess(7);
  assert.notEqual(ticket, undefined);
  original.host.completeScriptedAccess(7, ticket);
  original.registration.dispose();
});

test('owner registration rejects a capability without binding authority', () => {
  assert.throws(
    () => manager.registerFlashOwner('missing-binding-authority:g1', {
      owner_identity: () => 'missing-binding-authority:g1',
    }),
    /binding-generation authority/,
  );
});

test('capability failure unregisters the retired generation for reuse', () => {
  const key = 'failed-owner:g1';
  let fail = false;
  const capability = {
    owner_identity: () => key,
    ...fakeBindingAuthority(),
    set_flash_scripted_access_pending: () => { if (fail) throw new Error('retired'); },
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };
  const first = manager.registerFlashOwner(key, capability);
  fail = true;
  first.host.beginScriptedAccess(7);
  assert.equal(first.host.disposed, true);
  const replacement = manager.registerFlashOwner(key, {
    ...capability,
    set_flash_scripted_access_pending: () => {},
  });
  replacement.dispose();
});

test('late old registration disposal preserves same-key replacement resources', () => {
  const ownerKey = 'same-key-replacement:g1';
  const bindingState = {};
  const makeCapability = () => ({
    owner_identity: () => ownerKey,
    ...fakeBindingAuthority(bindingState),
    set_flash_scripted_access_pending: () => {},
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  });
  const oldBridge = manager.initFlashBridge(makeCapability());
  const oldState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  const oldPlayer = installReadyOwnerInstance(oldBridge.host, 7, oldState);

  // Direct host disposal retires the registry entry but leaves resource
  // cleanup to the registration disposer, matching a reentrant teardown.
  oldBridge.host.dispose();
  const replacementBridge = manager.initFlashBridge(makeCapability());
  const replacementState = { gets: [], sets: [], calls: [], gotos: [], hitTests: [] };
  oldPlayer.remove = () => {
    installReadyOwnerInstance(replacementBridge.host, 7, replacementState);
  };

  // The old registration runs after the replacement exists. Its remove hook
  // also publishes a replacement while teardown is in progress.
  oldBridge();
  assert.equal(replacementBridge.host.instances.has(`${ownerKey}:7`), true);
  assert.equal(window.dirplayer_flashInstances.has(`${ownerKey}:7`), true);
  replacementBridge();
});

test('production owner routing drops a disposed owner on non-LIFO teardown', () => {
  const makeOwner = (ownerKey) => {
    let host;
    const capability = {
      owner_identity: () => ownerKey,
      ...fakeBindingAuthority(),
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

test('owner scripted access waits are counted per host and survive partial completion', () => {
  const a = makeCapability('scripted-access-a:g1', {});
  const b = makeCapability('scripted-access-b:g1', {});

  a.host.beginScriptedAccess(1);
  a.host.beginScriptedAccess(2);
  b.host.beginScriptedAccess(7);
  assert.equal(a.host.scriptedAccessRequestCount(), 2);
  assert.equal(a.state.scriptedAccessPending, true);
  assert.equal(b.state.scriptedAccessPending, true);

  a.host.completeScriptedAccess(1);
  assert.equal(a.host.scriptedAccessRequestCount(), 1);
  assert.equal(a.state.scriptedAccessPending, true, 'one completion cannot clear another wait');
  assert.equal(b.state.scriptedAccessPending, true, 'owners do not share blocking state');

  a.host.completeScriptedAccess(2);
  assert.equal(a.host.scriptedAccessRequestCount(), 0);
  assert.equal(a.state.scriptedAccessPending, false);
  assert.equal(b.state.scriptedAccessPending, true);
  b.host.completeScriptedAccess(7);
  assert.equal(b.state.scriptedAccessPending, false);

  a.registration.dispose();
  a.host.beginScriptedAccess(99);
  assert.equal(a.state.scriptedAccessPending, false, 'disposed generation rejects stale notifications');
  b.registration.dispose();
});

test('owned completion tickets cannot retire a different scripted wait', () => {
  const a = makeCapability('ticket-owner:g1', {});
  const first = a.host.beginScriptedAccess(7);
  const second = a.host.beginScriptedAccess(7);
  assert.notEqual(first, undefined);
  assert.notEqual(second, undefined);
  a.host.completeScriptedAccess(7, second);
  assert.equal(a.host.scriptedAccessRequestCount(), 1);
  a.host.completeScriptedAccess(7, first);
  assert.equal(a.host.scriptedAccessRequestCount(), 0);
  a.registration.dispose();
});

test('owned Flash operations queue before instance creation and drain only their owner', async () => {
  const a = makeCapability('before-instance-a:g1', {});
  const b = makeCapability('before-instance-b:g1', {});

  assert.equal(manager.getVariableForOwner(a.host, 7, '_level0.ready'), null);
  assert.equal(manager.getVariableForOwner(a.host, 7, '_level0.second'), null);
  assert.equal(manager.setVariableForOwner(a.host, 7, '_level0.ready', '1'), true);
  assert.equal(manager.callFunctionForOwner(a.host, 7, '_level0.boot', '[]'), null);
  manager.goToFrameForOwner(a.host, 7, 'warm0');
  manager.playFlashForOwner(a.host, 7);
  assert.equal(a.host.scriptedAccessRequestCount(), 5, 'two reads and three other scripted operations each retain a ticket');
  assert.equal(b.host.scriptedAccessRequestCount(), 0);

  assert.equal(manager.setVariableForOwner(b.host, 7, '_level0.other', '2'), true);
  assert.equal(b.host.scriptedAccessRequestCount(), 1);

  ownerBeingCreated = 'before-instance-a:g1';
  await manager.createFlashInstanceForOwner(a.host, 7, 2, 3, swfFixture(), 32, 24);
  assert.equal(a.host.scriptedAccessRequestCount(), 0, 'ready replay must drain A only');
  assert.equal(b.host.scriptedAccessRequestCount(), 1, 'A readiness cannot clear B');
  assert.equal(a.host.pendingQueue.drainReady(7, 'before-instance-a:g1:7', true, true).length, 0);
  assert.equal(b.host.pendingQueue.drainReady(7, 'before-instance-b:g1:7', true, true).length, 1);

  a.registration.dispose();
  b.registration.dispose();
});

test('owned queued goto and gotoAndStop preserve their distinct frame intent', async () => {
  const playing = makeCapability('queued-frame-play:g1', {});
  manager.goToFrameForOwner(playing.host, 8, '3');
  ownerBeingCreated = 'queued-frame-play:g1';
  await manager.createFlashInstanceForOwner(playing.host, 8, 2, 3, swfFixture(), 32, 24, false);
  await new Promise((resolve) => queueMicrotask(resolve));
  assert.deepEqual(
    commandsForOwner('queued-frame-play:g1').map(({ method, args }) => [method, args]),
    [['GotoFrame', [3, false]]],
    'queued goto must seek to its requested frame while leaving playback enabled',
  );
  playing.registration.dispose();

  const stopped = makeCapability('queued-frame-stop:g1', {});
  manager.goToFrameAndStopForOwner(stopped.host, 8, '4');
  ownerBeingCreated = 'queued-frame-stop:g1';
  await manager.createFlashInstanceForOwner(stopped.host, 8, 2, 3, swfFixture(), 32, 24, true);
  await new Promise((resolve) => queueMicrotask(resolve));
  assert.deepEqual(
    commandsForOwner('queued-frame-stop:g1').map(({ method, args }) => [method, args]),
    [
      ['GotoFrame', [1, false]],
      ['GotoFrame', [1, true]],
      ['GotoFrame', [4, false]],
      ['GotoFrame', [4, true]],
      ['play', []],
    ],
    'queued gotoAndStop must preserve the requested frame and stop pin after initial pause setup',
  );
  const instance = stopped.host.instances.get('queued-frame-stop:g1:8');
  assert.ok(instance);
  assert.equal(instance.stopped, true);
  stopped.registration.dispose();
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
  assert.equal(a.state.scriptedAccessPending ?? false, false, 'display-only loads do not block the owner');
  assert.equal(b.state.scriptedAccessPending ?? false, false, 'display-only loads do not block another owner');

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

test('prepared generation creation is idempotent and exact retirement preserves g2', async () => {
  const owner = makeCapability('owner-prepared-idempotence:g1', {});
  const player = installReadyOwnerInstance(owner.host, 7, { gets: [], sets: [], calls: [], gotos: [], hitTests: [] });
  const g1 = owner.host.instanceGenerations.get(7);
  await manager.createFlashInstanceForOwner(owner.host, 7, 2, 3, swfFixture(), 32, 24, false, -1, g1);
  assert.equal(owner.host.instances.get('owner-prepared-idempotence:g1:7').rufflePlayer, player);

  const g2 = owner.host.reserveInstanceGeneration(7);
  manager.destroyFlashInstanceAtGeneration(owner.host, 7, g1);
  assert.equal(owner.host.instances.has('owner-prepared-idempotence:g1:7'), false);
  assert.equal(owner.host.isCurrentInstanceGeneration(7, g2), true);
  owner.registration.dispose();
});

test('generation-filtered pending operations settle retired scripted tickets', () => {
  const state = {};
  const owner = makeCapability('owner-pending-generations:g1', state);
  const g1 = owner.host.reserveInstanceGeneration(7);
  const ticket = owner.host.beginScriptedAccess(7);
  owner.host.pendingQueue.enqueueOwned(owner.host.ownerKey, 7, { kind: 'stop' }, ticket, g1);
  const g2 = owner.host.reserveInstanceGeneration(7);
  owner.host.pendingQueue.enqueueOwned(owner.host.ownerKey, 7, { kind: 'play' }, undefined, g2);

  manager.destroyFlashInstanceAtGeneration(owner.host, 7, g1);
  assert.equal(state.scriptedAccessPending, false);
  assert.deepEqual(
    owner.host.pendingQueue.drainReady(7, `${owner.host.ownerKey}:7`, true, false, g2).map((entry) => entry.op),
    [{ kind: 'play' }],
  );
  owner.registration.dispose();
});

test('prepared actions retain load resize unload order through registration delay', () => {
  const ownerKey = 'owner-prepared-order:g1';
  const seen = [];
  const swf = swfFixture();
  vmApi.onFlashMemberLoadedPrepared(7, 1, 1, swf, 2, 2, false, -1, ownerKey, 1);
  vmApi.onFlashMemberResized(7, 1, 4, 5, ownerKey);
  vmApi.onFlashMemberUnloadedAtGeneration(7, 1, ownerKey);
  const dispose = vmApi.registerVmCallbacks({
    onFlashMemberLoaded: (...args) => seen.push(['load', args[9]]),
    onFlashMemberResized: (...args) => seen.push(['resize', args[1]]),
    onFlashMemberUnloadedAtGeneration: (...args) => seen.push(['unload', args[1]]),
  }, ownerKey);
  assert.deepEqual(seen, [['load', 1], ['resize', 1], ['unload', 1]]);
  dispose();
});

test('prepared action tail survives one callback rebind, while reset and overflow retire it', () => {
  const rebindKey = 'owner-prepared-rebind:g1';
  const swf = swfFixture();
  vmApi.onFlashMemberLoadedPrepared(7, 1, 1, swf, 2, 2, false, -1, rebindKey, 1);
  vmApi.onFlashMemberResized(7, 1, 4, 5, rebindKey);
  vmApi.onFlashMemberUnloadedAtGeneration(7, 1, rebindKey);
  const seen = [];
  let reboundDispose;
  const firstDispose = vmApi.registerVmCallbacks({
    onFlashMemberLoaded: () => {
      seen.push('load');
      reboundDispose = vmApi.registerVmCallbacks({
        onFlashMemberResized: () => seen.push('resize'),
        onFlashMemberUnloadedAtGeneration: () => seen.push('unload'),
      }, rebindKey);
    },
  }, rebindKey);
  firstDispose();
  assert.deepEqual(seen, ['load', 'resize', 'unload']);
  reboundDispose();

  const enqueueDuringDrainKey = 'owner-prepared-enqueue-during-drain:g1';
  const enqueueSeen = [];
  vmApi.onFlashMemberLoadedPrepared(7, 1, 1, swf, 2, 2, false, -1, enqueueDuringDrainKey, 1);
  const enqueueDispose = vmApi.registerVmCallbacks({
    onFlashMemberLoaded: () => {
      enqueueSeen.push('load');
      vmApi.onFlashMemberResized(7, 1, 4, 5, enqueueDuringDrainKey);
    },
    onFlashMemberResized: () => enqueueSeen.push('resize'),
  }, enqueueDuringDrainKey);
  assert.deepEqual(enqueueSeen, ['load', 'resize']);
  enqueueDispose();

  const boundedKey = 'owner-prepared-drain-budget:g1';
  const boundedSeen = [];
  vmApi.onFlashMemberLoadedPrepared(7, 1, 1, swf, 2, 2, false, -1, boundedKey, 1);
  const boundedDispose = vmApi.registerVmCallbacks({
    onFlashMemberLoaded: () => {
      boundedSeen.push('load');
      vmApi.onFlashMemberLoadedPrepared(7, 1, 1, swf, 2, 2, false, -1, boundedKey, 1);
    },
  }, boundedKey);
  assert.equal(boundedSeen.length, 128, 'self-feeding drain stops at its work budget');
  const boundedLate = [];
  vmApi.onFlashMemberLoadedPrepared(7, 1, 1, swf, 2, 2, false, -1, boundedKey, 1);
  const boundedLateDispose = vmApi.registerVmCallbacks({
    onFlashMemberLoaded: () => boundedLate.push('unexpected'),
  }, boundedKey);
  assert.deepEqual(boundedLate, []);
  boundedLateDispose();
  boundedDispose();

  const resetKey = 'owner-prepared-reset:g1';
  vmApi.onFlashMemberLoadedPrepared(7, 1, 1, swf, 2, 2, false, -1, resetKey, 1);
  vmApi.onFlashResetAll(resetKey);
  vmApi.onFlashMemberResized(7, 1, 4, 5, resetKey);
  const resetSeen = [];
  const resetDispose = vmApi.registerVmCallbacks({ onFlashMemberLoaded: () => resetSeen.push('late') }, resetKey);
  assert.deepEqual(resetSeen, []);
  resetDispose();

  const overflowKey = 'owner-prepared-overflow:g1';
  for (let index = 0; index < 129; index += 1) {
    vmApi.onFlashMemberLoadedPrepared(7, 1, 1, swf, 2, 2, false, -1, overflowKey, 1);
  }
  const overflowSeen = [];
  const overflowDispose = vmApi.registerVmCallbacks({ onFlashMemberLoaded: () => overflowSeen.push('unexpected') }, overflowKey);
  assert.deepEqual(overflowSeen, []);
  overflowDispose();
});

test('instance generation exhaustion fails without wrapping to a live alias', () => {
  const state = {};
  const owner = makeCapability('owner-generation-exhausted:g1', state);
  state.bindingState.nextGeneration = Number.MAX_SAFE_INTEGER;
  assert.throws(
    () => owner.host.reserveInstanceGeneration(3),
    /Flash instance generation exhausted/,
  );
  assert.equal(owner.host.instanceGenerations.has(3), false);
  owner.registration.dispose();
});

test('same-owner re-registration keeps capability generations monotonic', () => {
  const key = 'owner-generation-reregister:g1';
  const sharedState = {};
  const first = makeCapability(key, sharedState);
  const firstGeneration = first.host.reserveInstanceGeneration(7);
  first.registration.dispose();

  const replacement = makeCapability(key, sharedState);
  const replacementGeneration = replacement.host.reserveInstanceGeneration(7);
  assert.equal(replacementGeneration > firstGeneration, true);
  assert.equal(
    replacement.host.isCurrentInstanceGeneration(7, firstGeneration),
    false,
  );
  assert.equal(
    replacement.host.isCurrentInstanceGeneration(7, replacementGeneration),
    true,
  );
  assert.equal(
    replacement.host.capability.invalidate_flash_instance_generation(7, firstGeneration),
    false,
    'a late old-host invalidation cannot retire the replacement generation',
  );
  assert.equal(
    replacement.host.isCurrentInstanceGeneration(7, replacementGeneration),
    true,
  );
  replacement.registration.dispose();
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
      ...fakeBindingAuthority(),
      update_flash_frame: () => {},
      trigger_lingo_callback_on_script: () => false,
      local_connection_send: () => false,
      dispatch_flash_event: () => false,
      dispatch_flash_lingo: async () => false,
    };

    bridge.dirplayer_registerFlashOwner(key, capability);
    bridge.dirplayer_unregisterFlashOwner(key);
    releaseBundle();
    // Await the exact module URL used by dirplayer-js-api.js before removing
    // the delayed fixture.  Under the full suite the template's cached
    // _flashManagerPromise can still be resolving after two timer turns;
    // deleting the fixture then converts the real manager import into the
    // no-op fallback and poisons later owner-route tests.
    const importedManager = await import(bundlePath.href);
    assert.equal(typeof importedManager.registerFlashOwner, 'function');
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
    ...fakeBindingAuthority(),
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

test('same-owner replacement invalidates an unpublished Flash generation', async () => {
  const bridge = await import('../vm-rust/tests/browser_templates/dirplayer-js-api.js');
  const key = 'owner-replace-flight:g1';
  const capability = {
    owner_identity: () => key,
    ...fakeBindingAuthority(),
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };

  bridge.dirplayer_registerFlashOwner(key, capability);
  // The production bridge does not expose its host; the owner registry and
  // instance map are the lifecycle boundary exercised by this fixture.
  assert.equal(typeof window.dirplayer_ruffleGetVariableOwned, 'function');
  ownerBeingCreated = key;
  const removedBefore = removedPlayers.length;
  holdLoadResponses = true;
  bridge.onFlashMemberLoaded(9, 2, 3, swfFixture(), 32, 24, false, -1, key);
  for (let i = 0; i < 20 && heldLoadResponses.length < 1; i += 1) {
    await new Promise((resolve) => realSetTimeout(resolve, 0));
  }
  assert.equal(heldLoadResponses.length >= 1, true, 'first generation reached the controlled load wait');

  bridge.onFlashMemberLoaded(9, 2, 3, swfFixture(), 32, 24, false, -1, key);
  for (let i = 0; i < 20 && heldLoadResponses.length < 2; i += 1) {
    await new Promise((resolve) => realSetTimeout(resolve, 0));
  }
  assert.equal(heldLoadResponses.length >= 2, true, 'replacement generation reached the controlled load wait');

  holdLoadResponses = false;
  for (const release of heldLoadResponses.splice(0)) release();
  for (let i = 0; i < 40 && !(window.dirplayer_flashInstances?.has(`${key}:9`)); i += 1) {
    await new Promise((resolve) => realSetTimeout(resolve, 0));
  }
  const replacement = window.dirplayer_flashInstances?.get(`${key}:9`);
  assert.ok(replacement, 'replacement generation was published');
  assert.equal(replacement.instanceGeneration, 2);
  assert.equal(removedPlayers.length > removedBefore, true, 'stale first generation was disposed after its load completed');
  bridge.dirplayer_unregisterFlashOwner(key);
  await new Promise((resolve) => realSetTimeout(resolve, 0));
  assert.equal(window.dirplayer_flashInstances?.has(`${key}:9`) ?? false, false);
});

test('unloading a pending Flash creation invalidates its reserved generation', async () => {
  const bridge = await import('../vm-rust/tests/browser_templates/dirplayer-js-api.js');
  const key = 'owner-unload-flight:g1';
  const capability = {
    owner_identity: () => key,
    ...fakeBindingAuthority(),
    update_flash_frame: () => {},
    trigger_lingo_callback_on_script: () => false,
    local_connection_send: () => false,
    dispatch_flash_event: () => false,
    dispatch_flash_lingo: async () => false,
  };

  bridge.dirplayer_registerFlashOwner(key, capability);
  ownerBeingCreated = key;
  holdLoadResponses = true;
  bridge.onFlashMemberLoaded(10, 2, 3, swfFixture(), 32, 24, false, -1, key);
  for (let i = 0; i < 20 && heldLoadResponses.length < 1; i += 1) {
    await new Promise((resolve) => realSetTimeout(resolve, 0));
  }
  assert.equal(heldLoadResponses.length >= 1, true, 'pending unload creation reached the controlled load wait');

  bridge.onFlashMemberUnloaded(10, key);
  holdLoadResponses = false;
  for (const release of heldLoadResponses.splice(0)) release();
  await new Promise((resolve) => realSetTimeout(resolve, 10));
  for (let i = 0; i < 40 && (window.dirplayer_flashInstances?.has(`${key}:10`)); i += 1) {
    await new Promise((resolve) => realSetTimeout(resolve, 0));
  }
  assert.equal(window.dirplayer_flashInstances?.has(`${key}:10`) ?? false, false);
  bridge.dirplayer_unregisterFlashOwner(key);
});

test('owner-qualified bridge callbacks remain routed after a later owner registers', async () => {
  const bridge = await import('../vm-rust/tests/browser_templates/dirplayer-js-api.js');
  const calls = { a: 0, b: 0 };
  const makeCapability = (key, slot) => ({
    owner_identity: () => key,
    ...fakeBindingAuthority(),
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
