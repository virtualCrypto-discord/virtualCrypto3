import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { randomUUID } from "node:crypto";
import { HttpError, itemHash, text } from "./store.mjs";
import { preparationPrompt } from "./prompt.mjs";

const MAX_TEXT = 150000;
const limited = (value, max = MAX_TEXT) => {
  const result = String(value ?? "");
  return result.length > max
    ? result.slice(0, max) + "\n…（長い出力を省略）"
    : result;
};

// Bidirectional, newline-delimited JSON-RPC over a private child process's stdio.
// No shell interpolation, open TCP Codex endpoint, or credentials sent to the UI.
export class CodexClient extends EventEmitter {
  constructor({
    binary = process.env.REVIEW_CODEX_BIN || "codex",
    spawnProcess = spawn,
    cwd,
  } = {}) {
    super();
    this.binary = binary;
    this.spawnProcess = spawnProcess;
    this.cwd = cwd;
    this.pending = new Map();
    this.nextId = 0;
    this.connection = null;
    this.child = null;
    this.info = { connected: false, authenticated: false };
  }
  async connect() {
    if (this.connection) return this.connection;
    this.connection = this.open().catch((error) => {
      this.close();
      throw error;
    });
    return this.connection;
  }
  async open() {
    const env = {};
    for (const key of [
      "PATH",
      "HOME",
      "USER",
      "LOGNAME",
      "CODEX_HOME",
      "LANG",
      "LC_ALL",
      "TMPDIR",
      "TERM",
      "SSL_CERT_FILE",
      "SSL_CERT_DIR",
      "HTTPS_PROXY",
      "HTTP_PROXY",
      "NO_PROXY",
      "OPENAI_API_KEY",
      "CODEX_API_KEY",
    ]) {
      if (process.env[key] !== undefined) env[key] = process.env[key];
    }
    const child = this.spawnProcess(
      this.binary,
      ["app-server", "--listen", "stdio://"],
      {
        cwd: this.cwd,
        env,
        stdio: ["pipe", "pipe", "pipe"],
        detached: process.platform !== "win32",
      },
    );
    this.child = child;
    child.stdin.on("error", (error) => this.fail(error));
    let buffer = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      buffer += chunk;
      if (buffer.length > 8 * 1024 * 1024) {
        this.fail(new Error("Codexの応答が大きすぎます。"));
        this.close();
        return;
      }
      let end;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, end);
        buffer = buffer.slice(end + 1);
        if (!line.trim()) continue;
        try {
          this.receive(JSON.parse(line));
        } catch (error) {
          this.fail(new Error(`Codexとの通信に失敗しました: ${error.message}`));
          this.close();
          return;
        }
      }
    });
    // Drain stderr, but do not broadcast the user's local Codex configuration.
    child.stderr.on("data", () => {});
    child.on("error", (error) =>
      this.fail(
        new Error(
          error.code === "ENOENT"
            ? "Codex CLIが見つかりません。codexをインストールし、codex loginを実行してください。"
            : error.message,
        ),
      ),
    );
    child.on("exit", (code, signal) => {
      if (this.child !== child) return;
      this.child = null;
      this.connection = null;
      this.fail(
        new Error(
          `Codex接続が終了しました (${signal ?? code})。再接続してください。`,
        ),
      );
    });
    await this.request("initialize", {
      clientInfo: { name: "virtualcrypto_review", version: "0.1.0" },
      capabilities: { experimentalApi: true },
    });
    this.send({ method: "initialized", params: {} });
    const account = await this.request("account/read", { refreshToken: false });
    this.info = { connected: true, authenticated: Boolean(account.account) };
    this.emit("status", this.info);
    return this.info;
  }
  receive(message) {
    if (message.method && Object.hasOwn(message, "id"))
      this.emit("request", message);
    else if (message.method) this.emit("notification", message);
    else if (Object.hasOwn(message, "id")) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      if (message.error)
        pending.reject(
          new Error(message.error.message ?? "Codexの要求が失敗しました。"),
        );
      else pending.resolve(message.result);
    }
  }
  send(message) {
    if (!this.child?.stdin.writable)
      throw new Error("Codexに接続されていません。");
    this.child.stdin.write(JSON.stringify(message) + "\n", (error) => {
      if (error) this.fail(error);
    });
  }
  request(method, params) {
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`Codex ${method}が時間内に応答しませんでした。`));
      }, 60000);
      timer.unref();
      this.pending.set(id, { resolve, reject, timer });
      try {
        this.send({ id, method, params });
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(error);
      }
    });
  }
  fail(error) {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
    this.info = { connected: false, authenticated: false };
    this.emit("failure", error);
  }
  close() {
    const child = this.child;
    this.child = null;
    this.connection = null;
    this.fail(new Error("Codex接続を終了しました。"));
    if (!child) return;
    try {
      if (process.platform !== "win32" && child.pid)
        process.kill(-child.pid, "SIGTERM");
      else child.kill("SIGTERM");
    } catch {
      child.kill("SIGTERM");
    }
  }
}

export class PreparationManager extends EventEmitter {
  constructor({ client, store, root, snapshot }) {
    super();
    this.client = client;
    this.store = store;
    this.root = root;
    this.snapshot = snapshot;
    this.active = null;
    this.flushTimer = null;
    this.filePreviews = new Map();
    client.on("notification", (message) =>
      this.guard(() => this.notification(message)),
    );
    client.on("request", (message) =>
      this.guard(() => this.serverRequest(message)),
    );
    client.on("failure", (error) => {
      if (this.active) this.finish("interrupted", error.message);
      this.emit("change");
    });
    client.on("status", () => this.emit("change"));
  }
  guard(fn) {
    try {
      fn();
    } catch (error) {
      if (this.active) {
        try {
          this.finish("failed", error.message);
        } catch {
          this.active = null;
        }
      }
      this.emit("change");
    }
  }
  async start(id, body) {
    if (this.active)
      throw new HttpError(
        409,
        `${this.active.itemId}の準備が進行中です。完了または中断してから依頼してください。`,
      );
    const item = this.store.item(id),
      record = this.store.record(id),
      repository = this.snapshot();
    const instruction = text(body.instruction ?? "", "追加依頼", 10000);
    if (!["read-only", "workspace-write"].includes(body.mode))
      throw new HttpError(400, "準備モードが不正です。");
    if (body.fingerprint !== repository.fingerprint)
      throw new HttpError(
        409,
        "対象コードが変わりました。再読み込みしてください。",
      );
    const run = {
      id: randomUUID(),
      itemId: id,
      mode: body.mode,
      status: "starting",
      startedAt: new Date().toISOString(),
      head: repository.head,
      fingerprint: repository.fingerprint,
      itemHash: itemHash(item),
      threadId: null,
      turnId: null,
      report: "",
      events: [],
      requests: [],
      error: null,
      instruction,
    };
    this.filePreviews.clear();
    this.active = run;
    this.flush();
    try {
      const info = await this.client.connect();
      if (this.active !== run) return run;
      if (!info.authenticated)
        throw new Error(
          "Codexにログインしていません。ターミナルで codex login を実行してください。",
        );
      const previous = record.runs.at(-1);
      const options = {
        cwd: this.root,
        approvalPolicy: "on-request",
        approvalsReviewer: "user",
        sandbox: body.mode,
        developerInstructions:
          "このセッションはレビューの確認準備を支援する。人間の判断記録の変更、本番デプロイ、実環境への書き込み、秘密値の読み出しは行わない。",
      };
      let result;
      if (body.continue === true && previous?.threadId) {
        // A failed resume is reported; silently starting a new conversation would
        // hide the loss of context from the reviewer.
        result = await this.client.request("thread/resume", {
          ...options,
          threadId: previous.threadId,
        });
      } else result = await this.client.request("thread/start", options);
      if (this.active !== run) return run;
      run.threadId = result.thread.id;
      run.status = "running";
      this.flush();
      const turn = await this.client.request("turn/start", {
        threadId: run.threadId,
        cwd: this.root,
        approvalPolicy: "on-request",
        approvalsReviewer: "user",
        sandboxPolicy:
          body.mode === "read-only"
            ? { type: "readOnly", networkAccess: false }
            : {
                type: "workspaceWrite",
                writableRoots: [this.root],
                networkAccess: false,
              },
        input: [
          {
            type: "text",
            text: preparationPrompt(
              item,
              record,
              repository,
              instruction,
              body.mode,
            ),
          },
        ],
      });
      if (this.active === run) {
        run.turnId = turn.turn.id;
        this.flush();
      }
    } catch (error) {
      if (this.active === run) this.finish("failed", error.message);
    }
    return run;
  }
  log(kind, detail) {
    if (!this.active) return;
    this.active.events.push({
      at: new Date().toISOString(),
      kind,
      detail: limited(detail, 16000),
    });
    this.active.events = this.active.events.slice(-150);
    this.scheduleFlush();
  }
  scheduleFlush() {
    if (!this.flushTimer)
      this.flushTimer = setTimeout(() => this.guard(() => this.flush()), 200);
  }
  flush() {
    clearTimeout(this.flushTimer);
    this.flushTimer = null;
    if (this.active) this.store.saveRun(this.active.itemId, this.active);
    this.emit("change");
  }
  notification({ method, params: p = {} }) {
    const run = this.active;
    if (
      !run ||
      (p.threadId && p.threadId !== run.threadId) ||
      (p.turnId && run.turnId && p.turnId !== run.turnId)
    )
      return;
    if (method === "turn/started") {
      run.turnId = p.turn.id;
      run.status = "running";
      this.flush();
    } else if (method === "item/agentMessage/delta") {
      // Final item text is authoritative; deltas are only a live preview.
      run.preview = limited((run.preview ?? "") + (p.delta ?? ""));
      this.scheduleFlush();
    } else if (method === "item/started") {
      if (p.item.type === "agentMessage") run.preview = "";
      if (p.item.type === "fileChange")
        this.filePreviews.set(
          p.item.id,
          limited(JSON.stringify(p.item.changes, null, 2)),
        );
      if (p.item.type === "commandExecution")
        this.log("command", p.item.command);
      if (p.item.type === "mcpToolCall")
        this.log("tool", `${p.item.server}/${p.item.tool}`);
    } else if (method === "item/completed") {
      const item = p.item;
      if (item.type === "agentMessage") {
        run.report = limited(item.text);
        run.preview = "";
        this.log("message", item.text);
      } else if (item.type === "commandExecution")
        this.log(
          "result",
          `${item.command}\nexit: ${item.exitCode ?? "?"}\n${item.aggregatedOutput ?? ""}`,
        );
      else if (item.type === "fileChange")
        this.log("files", JSON.stringify(item.changes));
    } else if (method === "turn/plan/updated")
      this.log(
        "plan",
        (p.plan ?? []).map((step) => `${step.status}: ${step.step}`).join("\n"),
      );
    else if (method === "turn/diff/updated") {
      run.diff = limited(p.diff);
      this.scheduleFlush();
    } else if (method === "serverRequest/resolved") {
      run.requests = run.requests.filter(
        (request) => request.rpcId !== p.requestId,
      );
      if (!run.requests.length && run.status === "waiting")
        run.status = "running";
      this.flush();
    } else if (method === "turn/completed") {
      const status = p.turn.status;
      this.finish(
        status === "completed" && run.report
          ? "completed"
          : status === "interrupted"
            ? "interrupted"
            : "failed",
        p.turn.error?.message ??
          (status === "completed" && !run.report
            ? "準備結果が返されませんでした。"
            : null),
      );
    } else if (method === "error" || method === "warning")
      this.log("notice", p.error?.message ?? p.message);
  }
  serverRequest(message) {
    const run = this.active,
      p = message.params ?? {};
    const methods = [
      "item/commandExecution/requestApproval",
      "item/fileChange/requestApproval",
      "item/permissions/requestApproval",
      "item/tool/requestUserInput",
      "tool/requestUserInput",
    ];
    if (
      !run ||
      p.threadId !== run.threadId ||
      (run.turnId && p.turnId !== run.turnId) ||
      !methods.includes(message.method)
    ) {
      this.client.send({
        id: message.id,
        error: {
          code: -32601,
          message:
            "This review client does not handle this request in the current turn.",
        },
      });
      return;
    }
    run.requests.push({
      key: randomUUID(),
      rpcId: message.id,
      method: message.method,
      params: p,
      changes: this.filePreviews.get(p.itemId) ?? null,
    });
    run.status = "waiting";
    this.flush();
  }
  answer(key, body) {
    const run = this.active,
      request = run?.requests.find((r) => r.key === key);
    if (!request) throw new HttpError(409, "この確認要求は終了しています。");
    let result;
    if (request.method.endsWith("requestUserInput")) {
      const answers = Object.create(null);
      for (const question of request.params.questions) {
        const value = text(body.answers?.[question.id], "回答", 5000).trim();
        if (!value)
          throw new HttpError(400, "すべての質問に回答してください。");
        answers[question.id] = { answers: [value] };
      }
      result = { answers };
    } else {
      if (!["accept", "decline"].includes(body.decision))
        throw new HttpError(400, "承認の指定が不正です。");
      const available = request.params.availableDecisions;
      if (Array.isArray(available) && !available.includes(body.decision))
        throw new HttpError(400, "この承認要求では選べない判断です。");
      result =
        request.method === "item/permissions/requestApproval"
          ? {
              permissions:
                body.decision === "accept" ? request.params.permissions : {},
              scope: "turn",
            }
          : { decision: body.decision };
    }
    this.client.send({ id: request.rpcId, result });
    run.requests = run.requests.filter((r) => r !== request);
    if (!run.requests.length) run.status = "running";
    this.log(
      "decision",
      request.method.endsWith("requestUserInput")
        ? "人間が質問に回答しました。"
        : `人間の実行判断: ${body.decision}`,
    );
    this.flush();
  }
  finish(status, error = null) {
    if (!this.active) return;
    Object.assign(this.active, {
      status,
      error,
      endedAt: new Date().toISOString(),
      requests: [],
      preview: "",
    });
    this.flush();
    this.active = null;
    this.emit("change");
  }
  async interrupt() {
    const run = this.active;
    if (!run) throw new HttpError(409, "実行中の準備はありません。");
    if (run.threadId && run.turnId) {
      await this.client.request("turn/interrupt", {
        threadId: run.threadId,
        turnId: run.turnId,
      });
      // Keep the run active until turn/completed confirms interruption.
    } else {
      this.finish("interrupted", "開始処理を中断しました。");
      this.client.close();
    }
  }
  close() {
    if (this.active) this.finish("interrupted", "アプリを終了しました。");
    this.client.close();
    clearTimeout(this.flushTimer);
  }
}
