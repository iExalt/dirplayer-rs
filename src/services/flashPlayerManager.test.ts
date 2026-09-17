import { FlashPendingQueue, PendingOp } from "./flashPlayerManager";

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
  expect(queue.drainReady(7, "session-a:player-1:7", true, false)).toEqual([play]);
  expect(queue.drainReady(7, "session-b:player-1:7", true, false)).toEqual([stop]);

  // A legacy queue is consumed only when the sprite index points at the
  // instance becoming ready; an ambiguous owner must leave it queued.
  queue.enqueueLegacy(7, play);
  expect(queue.drainReady(7, "session-c:player-1:7", true, false)).toEqual([]);
  expect(queue.drainReady(7, undefined, true, true)).toEqual([play]);
});

test("owner retirement clears only that owner's pending Flash operations", () => {
  const queue = new FlashPendingQueue();
  queue.enqueueOwned("session-a:player-1", 7, play);
  queue.enqueueOwned("session-b:player-1", 7, stop);

  queue.clearOwner("session-a:player-1");

  expect(queue.drainReady(7, "session-a:player-1:7", true, false)).toEqual([]);
  expect(queue.drainReady(7, "session-b:player-1:7", true, false)).toEqual([stop]);
});
