import assert from "node:assert/strict";
import {
  onChannelChanged,
  onChannelDisplayNamesChanged,
  onDatumSnapshotOwned,
  onDebugMessageOwned,
  onScoreChanged,
  onScriptErrorOwned,
  onScriptInstanceSnapshotOwned,
  registerVmCallbacks,
} from "../dirplayer-js-api/index.js";

function callbacksFor(log) {
  return {
    onScoreChanged: (snapshot) => log.score.push(snapshot),
    onChannelChanged: (channel, snapshot) => log.channel.push([channel, snapshot]),
    onChannelDisplayNamesChanged: (names) => log.names.push(names),
    onDebugMessage: (message) => {
      if (message === "throw") throw new Error("owned callback failure");
      log.debug.push(message);
    },
    onScriptError: (data) => log.errors.push(data),
    onDatumSnapshot: (datumRef, snapshot) => log.datum.push([datumRef, snapshot]),
    onScriptInstanceSnapshot: (instanceId, snapshot) => log.instances.push([instanceId, snapshot]),
  };
}

const a = { score: [], channel: [], names: [], debug: [], errors: [], datum: [], instances: [] };
const b = { score: [], channel: [], names: [], debug: [], errors: [], datum: [], instances: [] };
const disposeA = registerVmCallbacks(callbacksFor(a), "owner-a");
const disposeB = registerVmCallbacks(callbacksFor(b), "owner-b");

onScoreChanged({ owner: "a" }, "owner-a");
onChannelChanged(2, { owner: "a" }, "owner-a");
onChannelDisplayNamesChanged({ owner: "a" }, "owner-a");
onScoreChanged({ owner: "b" }, "owner-b");
onChannelChanged(3, { owner: "b" }, "owner-b");
onChannelDisplayNamesChanged({ owner: "b" }, "owner-b");
onDebugMessageOwned("owner-a", "debug-a");
onScriptErrorOwned("owner-a", { message: "error-a", script_member_ref: [2, 3] });
onDatumSnapshotOwned("owner-a", 17, { type: "string", value: "datum-a" });
onScriptInstanceSnapshotOwned("owner-a", 23, { type: "object", value: "instance-a" });
onDebugMessageOwned("owner-b", "debug-b");
onScriptErrorOwned("owner-b", { message: "error-b" });
onDatumSnapshotOwned("owner-b", 19, { type: "int", value: 19 });
onScriptInstanceSnapshotOwned("owner-b", 29, { type: "object", value: "instance-b" });

assert.deepEqual(a.score, [{ owner: "a" }]);
assert.deepEqual(a.channel, [[2, { owner: "a" }]]);
assert.deepEqual(a.names, [{ owner: "a" }]);
assert.deepEqual(b.score, [{ owner: "b" }]);
assert.deepEqual(b.channel, [[3, { owner: "b" }]]);
assert.deepEqual(b.names, [{ owner: "b" }]);
assert.deepEqual(a.debug, ["debug-a"]);
assert.deepEqual(a.errors, [{ message: "error-a", script_member_ref: [2, 3] }]);
assert.deepEqual(a.datum, [[17, { type: "string", value: "datum-a" }]]);
assert.deepEqual(a.instances, [[23, { type: "object", value: "instance-a" }]]);
assert.deepEqual(b.debug, ["debug-b"]);
assert.deepEqual(b.errors, [{ message: "error-b" }]);
assert.deepEqual(b.datum, [[19, { type: "int", value: 19 }]]);
assert.deepEqual(b.instances, [[29, { type: "object", value: "instance-b" }]]);
assert.throws(() => onDebugMessageOwned("owner-b", "throw"), /owned callback failure/);

disposeA();
onScoreChanged({ owner: "retired-a" }, "owner-a");
onChannelChanged(4, { owner: "retired-a" }, "owner-a");
onChannelDisplayNamesChanged({ owner: "retired-a" }, "owner-a");
onDebugMessageOwned("owner-a", "retired-a");
onScriptErrorOwned("owner-a", { message: "retired-a" });
onDatumSnapshotOwned("owner-a", 41, { type: "void" });
onScriptInstanceSnapshotOwned("owner-a", 43, { type: "void" });
onDebugMessageOwned("owner-missing", "unknown");
onScoreChanged({ owner: "b2" }, "owner-b");
onChannelChanged(5, { owner: "b2" }, "owner-b");
onChannelDisplayNamesChanged({ owner: "b2" }, "owner-b");

assert.deepEqual(a.score, [{ owner: "a" }]);
assert.deepEqual(a.channel, [[2, { owner: "a" }]]);
assert.deepEqual(a.names, [{ owner: "a" }]);
assert.deepEqual(a.debug, ["debug-a"]);
assert.deepEqual(a.errors, [{ message: "error-a", script_member_ref: [2, 3] }]);
assert.deepEqual(a.datum, [[17, { type: "string", value: "datum-a" }]]);
assert.deepEqual(a.instances, [[23, { type: "object", value: "instance-a" }]]);
assert.deepEqual(b.score, [{ owner: "b" }, { owner: "b2" }]);
assert.deepEqual(b.channel, [[3, { owner: "b" }], [5, { owner: "b2" }]]);
assert.deepEqual(b.names, [{ owner: "b" }, { owner: "b2" }]);

disposeB();
console.log("owner-scoped VM callback routing: 2 owners + retired-owner rejection passed");
