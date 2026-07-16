import assert from "node:assert/strict";
import http from "node:http";
import net from "node:net";
import test from "node:test";

import { createApplication } from "../src/application.mjs";

function getText(url) {
  return new Promise((resolve, reject) => {
    const request = http.get(url, { agent: false }, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => {
        body += chunk;
      });
      response.on("end", () => resolve({ status: response.statusCode, body }));
    });
    request.once("error", reject);
  });
}

async function waitFor(predicate, message) {
  const deadline = Date.now() + 2_000;
  while (!predicate()) {
    assert.ok(Date.now() < deadline, message);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

function connectionFails(port) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      resolve(false);
    });
    socket.once("error", () => resolve(true));
    socket.setTimeout(500, () => {
      socket.destroy();
      resolve(true);
    });
  });
}

test("shutdown is idempotent, drains an active request, and releases resources", async () => {
  const records = [];
  const application = createApplication({
    logger: (record) => records.push(record),
    applicationShutdownMs: 1_000,
  });
  const address = await application.start();
  const origin = `http://127.0.0.1:${address.port}`;

  const health = await getText(`${origin}/health`);
  assert.equal(health.status, 200);

  const activeRequest = getText(`${origin}/hold?ms=150`);
  await waitFor(
    () => application.metrics().activeRequests === 1,
    "the hold request never became active",
  );

  const firstShutdown = application.shutdown("TEST");
  const secondShutdown = application.shutdown("TEST");
  assert.strictEqual(firstShutdown, secondShutdown);

  await assert.rejects(getText(`${origin}/health`));
  const held = await activeRequest;
  assert.deepEqual(held, { status: 200, body: "held" });
  await firstShutdown;

  assert.deepEqual(application.metrics(), {
    ready: false,
    activeRequests: 0,
    cleanupRuns: 1,
    keepAliveActive: false,
    listening: false,
  });
  assert.strictEqual(application.shutdown("TEST"), firstShutdown);
  assert.equal(await connectionFails(address.port), true);
  assert.deepEqual(
    records.map((record) => record.event),
    [
      "server_ready",
      "shutdown_started",
      "resource_cleanup_completed",
      "shutdown_completed",
    ],
  );
});
