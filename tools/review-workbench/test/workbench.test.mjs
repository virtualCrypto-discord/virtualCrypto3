import test from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { request as httpRequest } from "node:http";
import { spawn, execFileSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  rmSync,
  existsSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { catalog, categories } from "../lib/catalog.mjs";
import { Store, exportMarkdown, workspaceSnapshot } from "../lib/store.mjs";
import { preparationPrompt } from "../lib/prompt.mjs";
import { CodexClient, PreparationManager } from "../lib/codex.mjs";
import { createWorkbench } from "../server.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");
const repo = {
  head: "a".repeat(40),
  branch: "review",
  dirty: false,
  fingerprint: "first",
};
function temporary(t) {
  const dir = mkdtempSync(join(tmpdir(), "vc-review-test-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
function checkout(t) {
  const directory = temporary(t),
    workspace = join(directory, "repo");
  mkdirSync(workspace);
  execFileSync("git", ["init", "-q", "--initial-branch=main"], {
    cwd: workspace,
  });
  writeFileSync(join(workspace, "README.md"), "review fixture\n");
  writeFileSync(join(workspace, ".gitignore"), ".env\n.review-artifacts/\n");
  execFileSync("git", ["add", "."], { cwd: workspace });
  execFileSync(
    "git",
    [
      "-c",
      "user.name=Review Test",
      "-c",
      "user.email=review@example.invalid",
      "commit",
      "-qm",
      "fixture",
    ],
    { cwd: workspace },
  );
  return { directory, workspace };
}
const validDecision = (item = catalog[0], fingerprint = repo.fingerprint) => ({
  version: 0,
  fingerprint,
  status: "approved",
  reviewer: "Reviewer",
  notes: "両端末で確認した。",
  evidence: "テスト環境の結果を照合した。",
  checks: item.criteria.map(() => true),
});
async function until(predicate) {
  const deadline = Date.now() + 8000;
  while (!predicate()) {
    if (Date.now() > deadline) assert.fail("condition did not become true");
    await new Promise((r) => setTimeout(r, 10));
  }
}
function fixtureClient(mode, cwd = root) {
  return new CodexClient({
    cwd,
    spawnProcess: (_binary, _args, options) =>
      spawn(
        process.execPath,
        [join(here, "fixtures/codex-server.mjs"), mode],
        options,
      ),
  });
}

test("catalogue covers every project document and references real files", () => {
  assert.equal(catalog.length, 70);
  assert.equal(new Set(catalog.map((item) => item.id)).size, catalog.length);
  for (const item of catalog) {
    assert(categories.some((c) => c.id === item.category));
    assert(item.question && item.preparation && item.criteria.length >= 3);
    for (const path of item.sources)
      assert(existsSync(join(root, path)), `${item.id}: missing ${path}`);
  }
  for (const path of [
    "authorization",
    "contracts",
    "deploy",
    "issue",
    "known-gaps",
    "oauth2",
    "pat",
    "personal-grants",
    "qa",
    "renewal",
    "resources",
    "test-port",
    "web-ui",
  ]) {
    assert(
      catalog.some((item) => item.sources.includes(`docs/${path}.md`)),
      `uncovered document: ${path}`,
    );
  }
});

test("decisions require human evidence, persist, detect conflicts, and become stale", (t) => {
  const dir = temporary(t),
    store = new Store(dir),
    item = catalog[0];
  assert.throws(
    () => store.update(item.id, { ...validDecision(), evidence: "" }, repo),
    /根拠/,
  );
  assert.throws(
    () =>
      store.update(
        item.id,
        { ...validDecision(), checks: [false, true, true] },
        repo,
      ),
    /判断基準/,
  );
  assert.throws(
    () =>
      store.update(
        item.id,
        { ...validDecision(), status: "waived", notes: "" },
        repo,
      ),
    /判断メモ/,
  );
  store.update(item.id, validDecision(), repo);
  assert.equal(store.present(item, repo).stale, false);
  assert.throws(
    () => store.update(item.id, validDecision(), repo),
    (error) => error.status === 409,
  );
  assert.throws(
    () =>
      store.update(
        item.id,
        { ...validDecision(), version: 1 },
        { ...repo, fingerprint: "second" },
      ),
    /対象コード/,
  );
  const reopened = new Store(dir);
  assert.equal(reopened.record(item.id).status, "approved");
  assert.equal(reopened.record(item.id).history.length, 1);
  assert.equal(
    reopened.present(item, { ...repo, fingerprint: "second" }).stale,
    true,
  );
  assert.equal(
    reopened.present({ ...item, question: "new criteria" }, repo).stale,
    true,
  );
});

test("agent output never overwrites a human decision; restart marks unfinished preparations interrupted", (t) => {
  const dir = temporary(t),
    store = new Store(dir),
    item = catalog[0];
  store.update(item.id, validDecision(), repo);
  store.saveRun(item.id, {
    id: "run",
    status: "running",
    report: "confirm everything",
    requests: [{ key: "old" }],
  });
  const reopened = new Store(dir);
  assert.equal(reopened.record(item.id).status, "approved");
  assert.equal(reopened.record(item.id).version, 1);
  assert.equal(reopened.record(item.id).runs[0].status, "interrupted");
  assert.deepEqual(reopened.record(item.id).runs[0].requests, []);
});

test("corrupt saved data is never silently reset", (t) => {
  const dir = temporary(t);
  writeFileSync(join(dir, "state.json"), "broken");
  assert.throws(() => new Store(dir));
  assert.equal(readFileSync(join(dir, "state.json"), "utf8"), "broken");
});

test("fingerprint observes edits and untracked files, excludes secrets, and does not follow symlinks", (t) => {
  const { directory, workspace } = checkout(t);
  const before = workspaceSnapshot(workspace);
  writeFileSync(join(workspace, ".env"), "test-only-secret");
  assert.equal(workspaceSnapshot(workspace).fingerprint, before.fingerprint);
  writeFileSync(join(workspace, "README.md"), "edited");
  assert.notEqual(workspaceSnapshot(workspace).fingerprint, before.fingerprint);
  const edited = workspaceSnapshot(workspace).fingerprint;
  writeFileSync(join(workspace, "new.md"), "untracked");
  assert.notEqual(workspaceSnapshot(workspace).fingerprint, edited);
  const external = join(directory, "outside");
  writeFileSync(external, "one");
  symlinkSync(external, join(workspace, "link"));
  const withLink = workspaceSnapshot(workspace).fingerprint;
  writeFileSync(external, "two");
  assert.equal(workspaceSnapshot(workspace).fingerprint, withLink);
});

test("custom items and exports retain reasons, source revision and stale status", (t) => {
  const store = new Store(temporary(t));
  const item = store.add({
    title: "追加の判断",
    question: "現状の仕様でよいか",
    criteria: "対象を確認\n理由を記録",
  });
  store.update(
    item.id,
    {
      ...validDecision(item),
      status: "waived",
      checks: [false, false],
      notes: "今回の対象ではない",
    },
    repo,
  );
  const output = exportMarkdown(
    [store.present(item, { ...repo, fingerprint: "new" })],
    repo,
  );
  assert.match(output, /要再確認/);
  assert.match(output, /今回の対象ではない/);
  assert.match(output, new RegExp(repo.head));
  assert.throws(
    () => store.add({ title: "", question: "", criteria: "" }),
    /記入/,
  );
});

test("preparation prompt carries the concrete item, current revision and execution limits", () => {
  const prompt = preparationPrompt(
    catalog[0],
    { notes: "注目する点", evidence: "参考" },
    repo,
    "モバイルで試したい",
    "workspace-write",
  );
  for (const value of [
    catalog[0].question,
    ...catalog[0].criteria,
    repo.head,
    "注目する点",
    "モバイルで試したい",
    ".review-artifacts/UX-01/",
    "本番デプロイ",
    "実行していない",
  ])
    assert(prompt.includes(value));
});

for (const mode of ["approval", "question", "permissions", "early", "crash"]) {
  test(`stdio harness lifecycle: ${mode}`, async (t) => {
    const client = fixtureClient(mode),
      store = new Store(temporary(t));
    const manager = new PreparationManager({
      client,
      store,
      root,
      snapshot: () => repo,
    });
    t.after(() => manager.close());
    const run = await manager.start("UX-01", {
      instruction: "fixture",
      mode: "read-only",
      fingerprint: repo.fingerprint,
    });
    if (["approval", "question", "permissions"].includes(mode)) {
      await until(() => manager.active?.requests.length === 1);
      const request = manager.active.requests[0];
      assert.equal(manager.active.status, "waiting");
      assert.equal(store.record("UX-01").status, "todo");
      await assert.rejects(
        manager.start("UX-02", {
          mode: "read-only",
          fingerprint: repo.fingerprint,
        }),
        (e) => e.status === 409,
      );
      if (mode === "question")
        manager.answer(request.key, { answers: { environment: "ローカル" } });
      else manager.answer(request.key, { decision: "decline" });
      assert.throws(
        () => manager.answer(request.key, { decision: "accept" }),
        (e) => e.status === 409,
      );
    }
    await until(() => !manager.active);
    const saved = store.record("UX-01");
    assert.equal(saved.status, "todo");
    assert.equal(saved.runs.at(-1).id, run.id);
    assert.equal(
      saved.runs.at(-1).status,
      mode === "crash" ? "interrupted" : "completed",
    );
    if (mode === "approval") assert.match(saved.runs.at(-1).report, /decline/);
    if (mode === "question") assert.match(saved.runs.at(-1).report, /ローカル/);
    if (mode === "permissions")
      assert.match(
        saved.runs.at(-1).report,
        /"permissions":\{\},"scope":"turn"/,
      );
    if (mode !== "crash")
      assert.equal(store.present(catalog[0], repo).displayStatus, "ready");
  });
}

test("interrupting an approval does not approve it or leave a live request", async (t) => {
  const client = fixtureClient("approval"),
    store = new Store(temporary(t));
  const manager = new PreparationManager({
    client,
    store,
    root,
    snapshot: () => repo,
  });
  t.after(() => manager.close());
  await manager.start("UX-01", {
    mode: "read-only",
    fingerprint: repo.fingerprint,
  });
  await until(() => manager.active?.requests.length);
  await manager.interrupt();
  await until(() => !manager.active);
  assert.equal(store.record("UX-01").runs[0].status, "interrupted");
  assert.deepEqual(store.record("UX-01").runs[0].requests, []);
});

test("missing Codex executable is a recoverable error", async (t) => {
  const client = new CodexClient({
    binary: join(temporary(t), "absent"),
    cwd: root,
  });
  t.after(() => client.close());
  await assert.rejects(client.connect(), /Codex CLIが見つかりません/);
  assert.equal(client.info.connected, false);
});

test("file approvals include the proposed diff and unrelated requests are refused", async (t) => {
  const client = new EventEmitter(),
    sent = [];
  client.connect = async () => ({ authenticated: true });
  client.request = async (method) =>
    method === "thread/start"
      ? { thread: { id: "thread" } }
      : { turn: { id: "turn" } };
  client.send = (message) => sent.push(message);
  client.close = () => {};
  let manager;
  t.after(() => manager?.close());
  const store = new Store(temporary(t));
  manager = new PreparationManager({
    client,
    store,
    root,
    snapshot: () => repo,
  });
  await manager.start("UX-01", {
    mode: "workspace-write",
    fingerprint: repo.fingerprint,
  });
  const changes = [
    {
      path: ".review-artifacts/example.txt",
      kind: "add",
      diff: "+review fixture",
    },
  ];
  client.emit("notification", {
    method: "item/started",
    params: {
      threadId: "thread",
      turnId: "turn",
      item: { id: "patch", type: "fileChange", changes },
    },
  });
  client.emit("request", {
    id: 81,
    method: "item/fileChange/requestApproval",
    params: {
      threadId: "thread",
      turnId: "turn",
      itemId: "patch",
      reason: "prepare fixture",
    },
  });
  assert.match(manager.active.requests[0].changes, /review fixture/);
  const key = manager.active.requests[0].key;
  manager.answer(key, { decision: "decline" });
  assert.deepEqual(sent.at(-1), { id: 81, result: { decision: "decline" } });
  client.emit("request", {
    id: 82,
    method: "item/commandExecution/requestApproval",
    params: { threadId: "another-thread", turnId: "turn" },
  });
  assert.equal(sent.at(-1).error.code, -32601);
  client.emit("notification", {
    method: "turn/completed",
    params: {
      threadId: "another-thread",
      turn: { id: "turn", status: "completed" },
    },
  });
  assert(manager.active);
  client.emit("request", {
    id: 83,
    method: "item/tool/call",
    params: { threadId: "thread", turnId: "turn" },
  });
  assert.equal(sent.at(-1).error.code, -32601);
  assert.equal(store.record("UX-01").status, "todo");
});

test("HTTP decisions enforce origin, CSRF, source whitelist and optimistic updates", async (t) => {
  const { directory, workspace } = checkout(t);
  const client = new EventEmitter();
  client.info = { connected: false };
  client.close = () => {};
  const app = createWorkbench({
    root: workspace,
    dataDirectory: join(directory, "state"),
    client,
  });
  t.after(() => app.close());
  const { port } = await app.listen(0),
    origin = `http://127.0.0.1:${port}`;
  const state = await (await fetch(`${origin}/api/state`)).json();
  const payload = validDecision(catalog[0], state.repository.fingerprint);
  const patch = (headers, body = payload) =>
    fetch(`${origin}/api/items/UX-01`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify(body),
    });
  assert.equal((await patch({})).status, 403);
  assert.equal(
    (
      await patch({
        "X-Review-Token": state.token,
        Origin: "https://untrusted.invalid",
      })
    ).status,
    403,
  );
  const hostileHostStatus = await new Promise((resolveStatus, reject) => {
    const request = httpRequest(
      `${origin}/api/state`,
      { headers: { Host: "untrusted.invalid" } },
      (response) => {
        response.resume();
        resolveStatus(response.statusCode);
      },
    );
    request.on("error", reject);
    request.end();
  });
  assert.equal(hostileHostStatus, 403);
  assert.equal((await fetch(`${origin}/api/source?path=.env`)).status, 404);
  assert.equal(
    (await fetch(`${origin}/api/source?path=README.md`)).status,
    200,
  );
  assert.equal((await patch({ "X-Review-Token": state.token })).status, 200);
  assert.equal((await patch({ "X-Review-Token": state.token })).status, 409);
  const exported = await (
    await fetch(`${origin}/api/export?format=json`)
  ).json();
  assert.equal(exported.items[0].state.status, "approved");
  assert(!Object.hasOwn(exported, "token"));
  const custom = await fetch(`${origin}/api/items`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-Review-Token": state.token,
    },
    body: JSON.stringify({
      title: "<script>test</script>",
      question: "追加の確認",
      criteria: "確認する",
    }),
  });
  assert.equal(custom.status, 201);
  const page = await fetch(origin);
  assert.match(
    page.headers.get("content-security-policy"),
    /frame-ancestors 'none'/,
  );
  assert.throws(
    () =>
      createWorkbench({
        root: workspace,
        dataDirectory: join(directory, "state"),
        client,
      }),
    /すでに起動/,
  );
});
