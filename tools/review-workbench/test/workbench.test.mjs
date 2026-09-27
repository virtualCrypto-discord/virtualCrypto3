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
import {
  Store,
  exportMarkdown,
  workspaceSnapshot,
  workspaceDiff,
} from "../lib/store.mjs";
import { reviewPrompt } from "../lib/prompt.mjs";
import { CodexClient, ReviewManager } from "../lib/codex.mjs";
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

test("human decisions survive code and catalogue changes; only concurrent record updates conflict", (t) => {
  const dir = temporary(t),
    store = new Store(dir),
    item = catalog[0];
  assert.throws(
    () =>
      store.update(
        item.id,
        { ...validDecision(), checks: [false, true, true] },
        repo,
      ),
    /判断基準/,
  );
  store.update(item.id, validDecision(), repo);
  const saved = store.record(item.id);
  assert.throws(
    () => store.update(item.id, validDecision(), repo),
    (error) => error.status === 409,
  );
  const changed = {
    ...repo,
    head: "b".repeat(40),
    fingerprint: "second",
    dirty: true,
  };
  for (const definition of [
    item,
    {
      ...item,
      question: "Updated question",
      criteria: [...item.criteria].reverse(),
    },
  ]) {
    const shown = store.present(definition, changed);
    assert.equal(shown.displayStatus, "approved");
    assert.deepEqual(shown.state, saved);
  }
  // A form opened before a code change still saves normally.
  store.update(item.id, { ...validDecision(), version: 1 }, changed);
  const reopened = new Store(dir);
  assert.equal(reopened.record(item.id).status, "approved");
  assert.deepEqual(reopened.record(item.id).checks, saved.checks);
  assert.equal(reopened.record(item.id).history.length, 2);
  assert.equal(reopened.record(item.id).history[0].head, repo.head);
  assert.equal(reopened.record(item.id).history[1].head, changed.head);
});

test("all progress states can be saved with optional notes, evidence and reviewer left blank", (t) => {
  const store = new Store(temporary(t));
  for (const [version, status] of [
    "approved",
    "waived",
    "blocked",
    "changes",
    "reviewing",
    "todo",
  ].entries()) {
    const saved = store.update(
      "UX-01",
      {
        ...validDecision(),
        version,
        status,
        notes: "",
        evidence: "",
        reviewer: "",
      },
      repo,
    );
    assert.equal(saved.state.status, status);
    assert.equal(saved.state.notes, "");
    assert.equal(saved.state.evidence, "");
  }
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

test("custom items and exports preserve human decisions after source changes", (t) => {
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
  assert.match(output, /状態: 対象外/);
  assert.doesNotMatch(output, /要再確認|再確認が必要/);
  assert.match(output, /今回の対象ではない/);
  assert.match(output, new RegExp(repo.head));
  assert.throws(
    () => store.add({ title: "", question: "", criteria: "" }),
    /記入/,
  );
});

test("conversation prompt supports source changes, follow-ups and read-only consultation", () => {
  const prompt = reviewPrompt(
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
    "ソース・テスト・文書を修正",
    "回答の長さと形式は今回の依頼に合わせる",
  ])
    assert(prompt.includes(value));
  const consultation = reviewPrompt(
    catalog[0],
    {},
    repo,
    "まず相談したい",
    "read-only",
    "未保存の指摘",
  );
  assert.match(consultation, /読み取りのみ/);
  assert.match(consultation, /未保存の指摘/);
  assert.doesNotMatch(consultation, /ソース・テスト・文書を修正し/);
});

test("review conversation investigates, edits source, rechecks and retains the human decision", async (t) => {
  let manager;
  t.after(() => manager?.close());
  const { directory, workspace } = checkout(t);
  writeFileSync(join(workspace, "review-fixture.txt"), "label=old\n");
  const store = new Store(join(directory, "state"));
  const snapshot = () => workspaceSnapshot(workspace);
  const before = snapshot();
  store.update(
    "UX-01",
    { ...validDecision(catalog[0], before.fingerprint), status: "changes" },
    before,
  );
  const client = fixtureClient("conversation", workspace);
  manager = new ReviewManager({ client, store, root: workspace, snapshot });
  const send = async (instruction, options = {}) => {
    await manager.start("UX-01", {
      instruction,
      fingerprint: before.fingerprint,
      ...options,
    });
    await until(() => !manager.active);
    const run = store.record("UX-01").runs.at(-1);
    assert.equal(run.status, "completed", run.error);
    return run;
  };
  const investigation = await send("挙動を確認して", { mode: "read-only" });
  assert.match(investigation.report, /label=old/);
  const fix = await send("指摘したラベルを修正して、テストして", {
    context: "編集中の指摘",
  });
  assert.equal(fix.mode, "workspace-write");
  assert.equal(fix.threadId, investigation.threadId);
  assert.equal(fix.messages.length, 2);
  assert.equal(fix.context, "編集中の指摘");
  assert.match(fix.diff, /\+label=fixed/);
  assert.match(fix.events.find((e) => e.kind === "result").detail, /PASS/);
  assert.notEqual(fix.resultFingerprint, fix.fingerprint);
  assert.equal(fix.resultFingerprint, snapshot().fingerprint);
  assert.equal(
    readFileSync(join(workspace, "review-fixture.txt"), "utf8"),
    "label=fixed\n",
  );
  const pending = store.present(catalog[0], snapshot());
  assert.equal(pending.state.status, "changes");
  assert.deepEqual(pending.state.checks, validDecision().checks);
  assert.equal(pending.displayStatus, "ready");
  const recheck = await send("修正後をもう一度確認して", { mode: "read-only" });
  assert.equal(recheck.threadId, fix.threadId);
  assert.match(recheck.report, /label=fixed/);
  const fresh = await send("別の観点で相談したい", {
    mode: "read-only",
    continue: false,
  });
  assert.notEqual(fresh.threadId, fix.threadId);
  assert.equal(store.record("UX-01").runs.length, 4);
  const exported = exportMarkdown(
    [store.present(catalog[0], snapshot())],
    snapshot(),
  );
  for (const value of [
    "挙動を確認して",
    "label=old",
    "指摘したラベル",
    "+label=fixed",
    "もう一度確認して",
  ])
    assert(exported.includes(value));
  store.update(
    "UX-01",
    { ...validDecision(catalog[0], snapshot().fingerprint), version: 1 },
    snapshot(),
  );
  assert.equal(store.present(catalog[0], snapshot()).displayStatus, "approved");
  assert.equal(
    new Store(join(directory, "state")).record("UX-01").runs.length,
    4,
  );
});

test("current workspace diff includes staged and untracked edits, excluding ignored files and symlink targets", (t) => {
  const { directory, workspace } = checkout(t);
  writeFileSync(join(workspace, "README.md"), "tracked edit\n");
  execFileSync("git", ["add", "README.md"], { cwd: workspace });
  writeFileSync(join(workspace, "new.txt"), "new source\n");
  writeFileSync(join(workspace, ".env"), "ignored value");
  const external = join(directory, "outside");
  writeFileSync(external, "external value");
  symlinkSync(external, join(workspace, "external-link"));
  const diff = workspaceDiff(workspace);
  assert.match(diff, /tracked edit/);
  assert.match(diff, /new source/);
  assert.match(diff, /external-link/);
  assert.doesNotMatch(diff, /ignored value|external value/);
});

for (const [scenario, access, expected] of [
  ["approval", "workspace-write", "accept"],
  ["approval", "read-only", "decline"],
  ["permissions", "workspace-write", "enabled"],
  ["permissions", "read-only", 'permissions":{}'],
  ["question", "workspace-write"],
  ["early", "workspace-write"],
  ["crash", "workspace-write"],
]) {
  test(`stdio harness without execution prompts: ${scenario}/${access}`, async (t) => {
    let manager;
    t.after(() => manager?.close());
    const client = fixtureClient(scenario),
      store = new Store(temporary(t));
    manager = new ReviewManager({ client, store, root, snapshot: () => repo });
    const run = await manager.start("UX-01", {
      instruction: "fixture",
      mode: access,
    });
    if (scenario === "question") {
      await until(() => manager.active?.requests.length === 1);
      const request = manager.active.requests[0];
      assert.equal(manager.active.status, "waiting");
      assert.equal(store.record("UX-01").status, "todo");
      const queued = await manager.start("UX-02", { mode: access });
      assert.equal(queued.status, "queued");
      manager.cancelQueued(queued.id);
      manager.answer(request.key, { answers: { environment: "ローカル" } });
      assert.throws(
        () =>
          manager.answer(request.key, { answers: { environment: "ローカル" } }),
        (e) => e.status === 409,
      );
    }
    await until(() => !manager.active);
    const saved = store.record("UX-01");
    assert.equal(saved.status, "todo");
    assert.equal(saved.runs.at(-1).id, run.id);
    assert.equal(
      saved.runs.at(-1).status,
      scenario === "crash" ? "interrupted" : "completed",
    );
    assert.deepEqual(saved.runs.at(-1).requests, []);
    if (expected) assert(saved.runs.at(-1).report.includes(expected));
    if (scenario === "question")
      assert.match(saved.runs.at(-1).report, /ローカル/);
    if (scenario !== "crash")
      assert.equal(
        store.present(catalog[0], { ...repo, fingerprint: "new-code" })
          .displayStatus,
        "ready",
      );
  });
}

test("interrupting a question preserves progress and clears the pending request", async (t) => {
  const client = fixtureClient("question"),
    store = new Store(temporary(t));
  const manager = new ReviewManager({
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

test("legacy file approvals resolve automatically while unrelated requests are refused", async (t) => {
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
  manager = new ReviewManager({
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
  assert.match(manager.filePreviews.get("patch"), /review fixture/);
  assert.deepEqual(manager.active.requests, []);
  assert.deepEqual(sent.at(-1), { id: 81, result: { decision: "accept" } });
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
  writeFileSync(join(workspace, "README.md"), "updated during review\n");
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
