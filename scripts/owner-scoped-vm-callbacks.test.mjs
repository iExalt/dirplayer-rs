import assert from "node:assert/strict";
import {
  onChannelChanged,
  onChannelDisplayNamesChanged,
  onScoreChanged,
  registerVmCallbacks,
} from "../dirplayer-js-api/index.js";

function callbacksFor(log) {
  return {
    onScoreChanged: (snapshot) => log.score.push(snapshot),
    onChannelChanged: (channel, snapshot) => log.channel.push([channel, snapshot]),
    onChannelDisplayNamesChanged: (names) => log.names.push(names),
  };
}

const a = { score: [], channel: [], names: [] };
const b = { score: [], channel: [], names: [] };
const disposeA = registerVmCallbacks(callbacksFor(a), "owner-a");
const disposeB = registerVmCallbacks(callbacksFor(b), "owner-b");

onScoreChanged({ owner: "a" }, "owner-a");
onChannelChanged(2, { owner: "a" }, "owner-a");
onChannelDisplayNamesChanged({ owner: "a" }, "owner-a");
onScoreChanged({ owner: "b" }, "owner-b");
onChannelChanged(3, { owner: "b" }, "owner-b");
onChannelDisplayNamesChanged({ owner: "b" }, "owner-b");

assert.deepEqual(a.score, [{ owner: "a" }]);
assert.deepEqual(a.channel, [[2, { owner: "a" }]]);
assert.deepEqual(a.names, [{ owner: "a" }]);
assert.deepEqual(b.score, [{ owner: "b" }]);
assert.deepEqual(b.channel, [[3, { owner: "b" }]]);
assert.deepEqual(b.names, [{ owner: "b" }]);

disposeA();
onScoreChanged({ owner: "retired-a" }, "owner-a");
onChannelChanged(4, { owner: "retired-a" }, "owner-a");
onChannelDisplayNamesChanged({ owner: "retired-a" }, "owner-a");
onScoreChanged({ owner: "b2" }, "owner-b");
onChannelChanged(5, { owner: "b2" }, "owner-b");
onChannelDisplayNamesChanged({ owner: "b2" }, "owner-b");

assert.deepEqual(a.score, [{ owner: "a" }]);
assert.deepEqual(a.channel, [[2, { owner: "a" }]]);
assert.deepEqual(a.names, [{ owner: "a" }]);
assert.deepEqual(b.score, [{ owner: "b" }, { owner: "b2" }]);
assert.deepEqual(b.channel, [[3, { owner: "b" }], [5, { owner: "b2" }]]);
assert.deepEqual(b.names, [{ owner: "b" }, { owner: "b2" }]);

disposeB();
console.log("owner-scoped VM callback routing: 2 owners + retired-owner rejection passed");
