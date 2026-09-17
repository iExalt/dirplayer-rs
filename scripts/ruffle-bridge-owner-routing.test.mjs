import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const hostSource = await readFile(new URL('../public/dirplayer-ruffle-bridge-host.js', import.meta.url), 'utf8');

class TestEvent {
  constructor(type, init = {}) {
    this.type = type;
    this.detail = init.detail;
  }
}

class TestPlayer {
  constructor() {
    this.dataset = {};
    this.style = {};
    this.listeners = new Map();
    this.openUrlHandler = null;
    this.fsCommandHandler = null;
    this.removed = false;
    this.ownerKey = null;
    this.ownerBindings = [];
    this.loaded = false;
  }

  addEventListener(name, handler) {
    const handlers = this.listeners.get(name) ?? new Set();
    handlers.add(handler);
    this.listeners.set(name, handlers);
  }

  dirplayer_addOpenUrlHandler(handler) {
    this.openUrlHandler = handler;
  }

  dirplayer_addFSCommandHandler(handler) {
    this.fsCommandHandler = handler;
  }

  dirplayer_set_owner_key(ownerKey) {
    assert.equal(this.ownerKey, null, 'Ruffle element owner may be bound once');
    this.ownerKey = ownerKey;
    this.ownerBindings.push(ownerKey);
  }

  ruffle() {
    return { load: async () => { this.loaded = true; } };
  }

  remove() {
    this.removed = true;
  }
}

function makeBridgeHarness() {
  const listeners = new Map();
  const players = [];
  const body = { appendChild(node) { node.isConnected = true; return node; } };
  const document = {
    body,
    documentElement: { getAttribute() { return null; } },
    createElement() { return { textContent: '' }; },
    getElementById() { return null; },
  };
  const window = {
    __dirplayerRuffleBridgeInstalled: false,
    dirplayer_RufflePlayer: {
      newest() {
        return {
          createPlayer() {
            const player = new TestPlayer();
            players.push(player);
            return player;
          },
        };
      },
    },
    addEventListener(name, handler) {
      const handlers = listeners.get(name) ?? new Set();
      handlers.add(handler);
      listeners.set(name, handlers);
    },
    dispatchEvent(event) {
      for (const handler of listeners.get(event.type) ?? []) handler(event);
      return true;
    },
  };
  vm.runInNewContext(hostSource, {
    window,
    document,
    CustomEvent: TestEvent,
    Map,
    Set,
    Object,
    JSON,
    Promise,
    console,
  }, { filename: 'dirplayer-ruffle-bridge-host.js' });

  let requestId = 1;
  const events = [];
  window.addEventListener('dirplayer-ruffle-bridge-event', (event) => events.push(event.detail));
  const localConnectionEvents = [];
  window.addEventListener('dirplayer-lc-send', (event) => localConnectionEvents.push(event.detail));

  async function request(detail) {
    const id = requestId++;
    const response = new Promise((resolve) => {
      const handler = (event) => {
        if (event.detail.requestId !== id) return;
        resolve(event.detail);
      };
      window.addEventListener('dirplayer-ruffle-bridge-response', handler);
    });
    window.dispatchEvent(new TestEvent('dirplayer-ruffle-bridge-request', {
      detail: { requestId: id, ...detail },
    }));
    return response;
  }

  return { window, players, events, localConnectionEvents, request };
}

test('main-world bridge carries exact owner keys through callbacks and legacy LocalConnection', async () => {
  const harness = makeBridgeHarness();
  const aResponse = await harness.request({ method: 'createPlayer', ownerKey: 'owner-a:g1' });
  const bResponse = await harness.request({ method: 'createPlayer', ownerKey: 'owner-b:g1' });
  assert.equal(aResponse.error, null);
  assert.equal(bResponse.error, null);
  assert.deepEqual(harness.players.map((player) => player.ownerBindings), [
    ['owner-a:g1'],
    ['owner-b:g1'],
  ], 'extension bridge binds each Ruffle element before publication');
  const aId = aResponse.result.playerId;
  const bId = bResponse.result.playerId;
  await harness.request({ method: 'callMethod', playerId: aId, methodName: 'load', args: [{}] });
  await harness.request({ method: 'callMethod', playerId: bId, methodName: 'load', args: [{}] });
  assert.deepEqual(harness.players.map((player) => player.loaded), [true, true]);

  assert.equal((await harness.request({
    method: 'registerCallbackForwarders', playerId: aId, ownerKey: 'owner-a:g1',
  })).error, null);
  assert.equal((await harness.request({
    method: 'registerCallbackForwarders', playerId: bId, ownerKey: 'owner-b:g1',
  })).error, null);
  assert.match((await harness.request({
    method: 'registerCallbackForwarders', playerId: aId, ownerKey: 'owner-b:g1',
  })).error, /owner/);

  // This is the producer imported by the patched AVM1 LocalConnection path.
  // Both active generations must reach only their own owner-qualified event.
  assert.equal(harness.window.dirplayer_localConnectionSendOwned(
    'owner-a:g1', 'owned-a', 'm', '[]',
  ), false);
  assert.equal(harness.window.dirplayer_localConnectionSendOwned(
    'owner-b:g1', 'owned-b', 'm', '[]',
  ), false);
  assert.deepEqual(harness.localConnectionEvents.map((event) => event.ownerKey), [
    'owner-a:g1',
    'owner-b:g1',
  ]);

  harness.players[0].openUrlHandler('event:from-a', '_self');
  harness.players[1].openUrlHandler('event:from-b', '_self');
  assert.deepEqual(harness.events.map((event) => event.ownerKey), ['owner-a:g1', 'owner-b:g1']);

  // The legacy three-argument producer is preserved for one-owner pages, but
  // is rejected while owners are ambiguous instead of being broadcast.
  assert.equal(harness.window.dirplayer_localConnectionSend('c', 'm', '[]'), false);
  assert.equal(harness.localConnectionEvents.length, 2);

  await harness.request({ method: 'destroyPlayer', playerId: aId });
  assert.equal(harness.window.dirplayer_localConnectionSendOwned(
    'owner-a:g1', 'retired', 'm', '[]',
  ), false);
  assert.equal(harness.localConnectionEvents.length, 2, 'retired owner is rejected');
  assert.equal(harness.window.dirplayer_localConnectionSendOwned(
    'owner-b:g1', 'owned-b-live', 'm', '[]',
  ), false);
  assert.equal(harness.localConnectionEvents.length, 3, 'live owner remains routable');
  assert.equal(harness.window.dirplayer_localConnectionSend('c', 'm', '[]'), false);
  assert.equal(harness.localConnectionEvents.length, 4);
  assert.equal(harness.localConnectionEvents[3].ownerKey, 'owner-b:g1');
  assert.equal(harness.localConnectionEvents[3].name, 'c');
  assert.equal(harness.localConnectionEvents[3].method, 'm');
  assert.equal(harness.localConnectionEvents[3].argsJson, '[]');

  await harness.request({ method: 'destroyPlayer', playerId: bId });
  assert.equal(harness.window.dirplayer_localConnectionSend('c', 'm', '[]'), false);
  assert.equal(harness.localConnectionEvents.length, 4);
});
