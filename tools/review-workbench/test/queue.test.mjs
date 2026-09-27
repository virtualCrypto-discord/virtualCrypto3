import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { Store } from "../lib/store.mjs";
import { ReviewManager } from "../lib/codex.mjs";
import { createWorkbench } from "../server.mjs";
import { ControlledClient } from "./fixtures/controlled-client.mjs";

const repository = {
  head: "a".repeat(40),
  branch: "review",
  fingerprint: "first",
  dirty: false,
};
function temporary(t) {
  const dir = mkdtempSync(join(tmpdir(), "vc-review-queue-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
async function until(predicate) {
  const deadline = Date.now() + 5000;
  while (!predicate()) {
    if (Date.now() > deadline) assert.fail("queue did not advance");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}
function managerFor(
  t,
  directory,
  snapshot = () => repository,
  client = new ControlledClient(),
) {
  const store = new Store(directory);
  const manager = new ReviewManager({
    client,
    store,
    root: directory,
    snapshot,
  });
  t.after(() => manager.close());
  return { manager, store, client };
}

test("messages run FIFO across items, using current code, notes and each item's latest thread", async (t) => {
  let snapshot = repository;
  const { manager, store, client } = managerFor(
    t,
    temporary(t),
    () => snapshot,
  );
  const first = await manager.start("UX-01", { instruction: "first" });
  const second = await manager.start("UX-01", {
    instruction: "second",
    context: "unsaved note",
    mode: "read-only",
  });
  const third = await manager.start("UX-02", { instruction: "third" });
  const cancelled = await manager.start("UX-01", { instruction: "cancelled" });
  const fresh = await manager.start("UX-01", {
    instruction: "fresh",
    continue: false,
  });
  const continuation = await manager.start("UX-01", {
    instruction: "continuation",
  });
  assert.equal(second.status, "queued");
  assert.equal(client.turns, 1);
  manager.cancelQueued(cancelled.id);
  assert.deepEqual(
    store.queued().map((m) => m.id),
    [second.id, third.id, fresh.id, continuation.id],
  );
  assert.throws(
    () => manager.cancelQueued(cancelled.id),
    (e) => e.status === 409,
  );
  snapshot = { ...repository, fingerprint: "new-code", head: "b".repeat(40) };
  store.update(
    "UX-01",
    {
      version: 0,
      status: "changes",
      checks: [true, false, false],
      notes: "newly saved note",
      evidence: "",
      reviewer: "",
    },
    snapshot,
  );
  const oldTurn = client.complete();
  await until(() => manager.active?.id === second.id && manager.active.turnId);
  assert.equal(manager.active.threadId, first.threadId);
  assert.equal(manager.active.fingerprint, "new-code");
  const request = client.calls.findLast((c) => c.method === "turn/start");
  assert.equal(request.params.sandboxPolicy.type, "readOnly");
  for (const value of [
    snapshot.head,
    "newly saved note",
    "unsaved note",
    "second",
  ])
    assert(request.params.input[0].text.includes(value));
  assert.throws(
    () => manager.cancelQueued(second.id),
    (e) => e.status === 409,
  );
  // A delayed completion from the previous turn cannot finish the new one.
  client.emit("notification", {
    method: "turn/completed",
    params: {
      threadId: oldTurn.threadId,
      turn: { id: oldTurn.turnId, status: "completed" },
    },
  });
  assert.equal(manager.active.id, second.id);
  client.complete();
  await until(() => manager.active?.id === third.id && manager.active.turnId);
  assert.notEqual(manager.active.threadId, first.threadId);
  client.complete();
  await until(() => manager.active?.id === fresh.id && manager.active.turnId);
  const freshThread = manager.active.threadId;
  assert.notEqual(freshThread, first.threadId);
  client.complete();
  await until(
    () => manager.active?.id === continuation.id && manager.active.turnId,
  );
  assert.equal(manager.active.threadId, freshThread);
  client.complete();
  await until(() => !manager.active && !store.queued().length);
  assert.equal(client.turns, 5);
  assert.deepEqual(
    store.record("UX-01").runs.map((r) => r.instruction),
    ["first", "second", "fresh", "continuation"],
  );
  assert.equal(store.record("UX-01").status, "changes");
  assert.deepEqual(store.record("UX-01").checks, [true, false, false]);
});

test("waiting messages survive shutdown and automatically resume without replaying interrupted work", async (t) => {
  const dir = temporary(t);
  const { manager, store } = managerFor(t, dir);
  const first = await manager.start("UX-01", { instruction: "running" });
  const waiting = await manager.start("UX-01", {
    instruction: "after restart",
    context: "keep me",
    mode: "read-only",
  });
  manager.close();
  assert.equal(store.record("UX-01").runs[0].status, "interrupted");
  assert.equal(store.queued()[0].id, waiting.id);
  const reopened = managerFor(t, dir);
  await until(() => reopened.manager.active?.turnId);
  const active = reopened.manager.active;
  assert.equal(active.id, waiting.id);
  assert.equal(active.queuedAt, waiting.queuedAt);
  assert.equal(active.context, "keep me");
  assert.equal(active.threadId, first.threadId);
  assert.equal(active.mode, "read-only");
  assert.deepEqual(reopened.store.queued(), []);
  reopened.client.complete();
  assert.equal(reopened.store.record("UX-01").runs.length, 2);
  assert.equal(reopened.client.turns, 1);
});

test("messages queued during a failed connection start once, and interruption advances the queue", async (t) => {
  const client = new ControlledClient();
  let rejectConnection;
  const pending = new Promise((_, reject) => {
    rejectConnection = reject;
  });
  let connects = 0;
  client.connect = () =>
    ++connects === 1 ? pending : Promise.resolve(client.info);
  const { manager, store } = managerFor(
    t,
    temporary(t),
    () => repository,
    client,
  );
  const firstPromise = manager.start("UX-01", {
    instruction: "fails to start",
  });
  const second = await manager.start("UX-01", { instruction: "next" });
  const third = await manager.start("UX-02", { instruction: "last" });
  rejectConnection(new Error("connection failed"));
  const first = await firstPromise;
  assert.equal(first.status, "failed");
  await until(() => manager.active?.id === second.id && manager.active.turnId);
  assert.equal(client.turns, 1);
  await manager.interrupt();
  await until(() => manager.active?.id === third.id && manager.active.turnId);
  client.complete();
  assert.equal(client.turns, 2);
  assert.deepEqual(store.queued(), []);
  assert.deepEqual(
    store.record("UX-01").runs.map((r) => r.status),
    ["failed", "interrupted"],
  );
});

test("HTTP accepts queued messages while busy and protects cancellation from stale requests and CSRF", async (t) => {
  const dir = temporary(t);
  execFileSync("git", ["init", "-q"], { cwd: dir });
  writeFileSync(join(dir, "README.md"), "queue fixture\n");
  execFileSync("git", ["add", "README.md"], { cwd: dir });
  execFileSync(
    "git",
    [
      "-c",
      "user.name=Test",
      "-c",
      "user.email=test@example.invalid",
      "commit",
      "-qm",
      "fixture",
    ],
    { cwd: dir },
  );
  const client = new ControlledClient();
  const app = createWorkbench({
    root: dir,
    dataDirectory: join(dir, ".state"),
    client,
  });
  t.after(() => app.close());
  const { port } = await app.listen(0);
  const origin = `http://127.0.0.1:${port}`;
  const state = () => fetch(`${origin}/api/state`).then((r) => r.json());
  const { token } = await state();
  const post = (path, body = {}, useToken = true) =>
    fetch(`${origin}/api/${path}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(useToken ? { "X-Review-Token": token } : {}),
      },
      body: JSON.stringify(body),
    });
  await post("items/UX-01/messages", { instruction: "running" });
  const response = await post("items/UX-02/messages", {
    instruction: "<img src=x>queued",
    continue: false,
  });
  assert.equal(response.status, 202);
  const queued = await response.json();
  assert.equal(queued.status, "queued");
  assert.equal((await state()).queue[0].instruction, "<img src=x>queued");
  assert.equal(
    (await post("items/UX-02/messages", { mode: "invalid" })).status,
    400,
  );
  assert.equal((await state()).queue.length, 1);
  const path = `queue/${queued.id}/cancel`;
  assert.equal((await post(path, {}, false)).status, 403);
  assert.equal((await state()).queue.length, 1);
  assert.equal((await post(path)).status, 200);
  assert.equal((await post(path)).status, 409);
  assert.deepEqual((await state()).queue, []);
  client.complete();
  assert.equal(client.turns, 1);
});
