#!/usr/bin/env bun

// Small, dependency-free WebSocket fixture for the browser lifecycle test.
// Bun's built-in server binds port 0 so parallel test runs never share a
// fixed port. The state endpoint is intentionally plain JSON so the wasm
// test can verify close, B survival, and the absence of a stale A write.
const opened = [];
const closed = [];
const received = [];

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(request, server) {
    const url = new URL(request.url);
    if (url.pathname === "/state") {
      const state = {
        opened,
        closed,
        received,
        openedA: opened.includes("A"),
        openedB: opened.includes("B"),
        closedA: closed.includes("A"),
        receivedStaleA: received.some((entry) =>
          entry.label === "A" && entry.payload === "A-stale-after-reset"),
        receivedSurvivalB: received.some((entry) =>
          entry.label === "B" && entry.payload === "B-survives-A-reset"),
      };
      return new Response(JSON.stringify(state), {
        headers: {
          "Content-Type": "application/json",
          "Access-Control-Allow-Origin": "*",
        },
      });
    }
    if (url.pathname === "/fileio.txt") {
      return new Response("owner-bound fileio fixture\n", {
        headers: {
          "Content-Type": "text/plain",
          "Access-Control-Allow-Origin": "*",
        },
      });
    }
    if (server.upgrade(request, { data: { label: url.pathname.slice(1) || "unknown" } })) {
      return undefined;
    }
    return new Response("multiuser lifecycle fixture");
  },
  websocket: {
    open(socket) {
      opened.push(socket.data.label);
      if (socket.data.label === "A" || socket.data.label === "B") {
        socket.send(`${socket.data.label}-server-message`);
      }
    },
    message(socket, message) {
      const bytes = typeof message === "string"
        ? message
        : new TextDecoder().decode(new Uint8Array(message));
      received.push({ label: socket.data.label, payload: bytes });
      if (socket.data.label === "B" && bytes === "B-survives-A-reset") {
        socket.send("B-after-A-reset");
      }
    },
    close(socket) {
      closed.push(socket.data.label);
    },
  },
});

console.log(`MULTIUSER_TEST_SERVER_READY ${server.port}`);

process.on("SIGTERM", () => server.stop(true));
process.on("SIGINT", () => server.stop(true));
