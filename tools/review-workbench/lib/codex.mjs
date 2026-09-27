import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { randomUUID } from "node:crypto";
import { HttpError, itemHash, text } from "./store.mjs";
import { reviewPrompt } from "./prompt.mjs";

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

export class ReviewManager extends EventEmitter {
  constructor({ client, store, root, snapshot }) {
    super();
    this.client = client;
    this.store = store;
    this.root = root;
    this.snapshot = snapshot;
    this.active = null;
    this.closed = false;
    this.draining = false;
    this.drainTimer = null;
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
    this.scheduleDrain();
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
      this.client.close();
      this.emit("change");
    }
  }
  async start(id, body) {
    if (this.closed) throw new HttpError(503, "アプリを終了しています。");
    this.store.item(id);
    const instruction =
      text(body.instruction ?? "", "メッセージ", 10000).trim() ||
      "この項目を確認したいので準備してください。";
    const context = text(body.context ?? "", "編集中のメモ", 45000);
    const mode = body.mode ?? "workspace-write";
    if (!["read-only", "workspace-write"].includes(mode))
      throw new HttpError(400, "実行範囲が不正です。");
    const message = {
      id: randomUUID(),
      itemId: id,
      mode,
      instruction,
      context,
      continued: body.continue !== false,
      status: "queued",
      queuedAt: new Date().toISOString(),
    };
    this.store.enqueue(message);
    this.emit("change");
    const run = await this.drain();
    return run?.id === message.id ? run : message;
  }
  cancelQueued(id) {
    this.store.cancelQueued(id);
    this.emit("change");
  }
  scheduleDrain() {
    if (this.closed || this.drainTimer) return;
    this.drainTimer = setImmediate(() => {
      this.drainTimer = null;
      this.drain().catch((error) =>
        this.guard(() => {
          throw error;
        }),
      );
    });
  }
  async drain() {
    if (this.closed || this.active || this.draining) return;
    const message = this.store.queued()[0];
    if (!message) return;
    this.draining = true;
    try {
      return await this.execute(message);
    } finally {
      this.draining = false;
      if (!this.active && this.store.queued().length) this.scheduleDrain();
    }
  }
  async execute(message) {
    const { itemId: id, mode, instruction, context } = message;
    const run = {
      ...message,
      status: "starting",
      startedAt: new Date().toISOString(),
      threadId: null,
      turnId: null,
      report: "",
      messages: [],
      events: [],
      requests: [],
      error: null,
    };
    this.filePreviews.clear();
    this.active = run;
    this.flush();
    try {
      // Resolve the code, saved notes and previous conversation when execution
      // actually starts, after all earlier queued changes have finished.
      const item = this.store.item(id),
        record = this.store.record(id),
        repository = this.snapshot();
      Object.assign(run, {
        head: repository.head,
        fingerprint: repository.fingerprint,
        itemHash: itemHash(item),
      });
      this.flush();
      const info = await this.client.connect();
      if (this.active !== run) return run;
      if (!info.authenticated)
        throw new Error(
          "Codexにログインしていません。ターミナルで codex login を実行してください。",
        );
      const previous = record.runs.findLast((entry) => entry.threadId);
      const options = {
        cwd: this.root,
        approvalPolicy: "never",
        approvalsReviewer: "user",
        sandbox: mode === "read-only" ? "read-only" : "danger-full-access",
        developerInstructions:
          "人間とレビューを進める継続的な会話。依頼に応じて調査・準備・ソース修正・検証・再確認を実行する。作業範囲内の修正を依頼されたら、提案だけで止めない。最終的なレビュー判断は人間が行う。判断データの書き換え、本番デプロイ、外部への書き込み、秘密値の読み出しは行わない。",
      };
      let result;
      if (run.continued && previous?.threadId) {
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
        approvalPolicy: "never",
        approvalsReviewer: "user",
        sandboxPolicy:
          mode === "read-only"
            ? { type: "readOnly", networkAccess: false }
            : { type: "dangerFullAccess" },
        input: [
          {
            type: "text",
            text: reviewPrompt(
              item,
              record,
              repository,
              instruction,
              mode,
              context,
            ),
          },
        ],
      });
      if (this.active === run) {
        run.turnId = turn.turn.id;
        this.flush();
      }
    } catch (error) {
      if (this.active === run) {
        try {
          this.finish("failed", error.message);
        } finally {
          this.active = null;
          this.client.close();
        }
      }
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
    const turnId = p.turnId ?? p.turn?.id;
    if (
      !run ||
      (p.threadId && p.threadId !== run.threadId) ||
      (turnId && run.turnId && turnId !== run.turnId)
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
        run.messages.push({
          id: item.id,
          at: new Date().toISOString(),
          phase: item.phase ?? "final_answer",
          text: limited(item.text),
        });
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
            ? "回答が返されませんでした。"
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
    // Normal local work is already authorized. Older/custom harnesses may
    // still send an approval RPC despite approvalPolicy=never; resolve it
    // without turning it into another UI gate. Questions remain interactive.
    if (!message.method.endsWith("requestUserInput")) {
      const available = p.availableDecisions;
      const decision =
        run.mode === "read-only"
          ? "decline"
          : !Array.isArray(available) || available.includes("accept")
            ? "accept"
            : available.includes("acceptForSession")
              ? "acceptForSession"
              : "decline";
      this.answer(run.requests.at(-1).key, { decision }, true);
      return;
    }
    this.flush();
  }
  answer(key, body, automatic = false) {
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
      if (!["accept", "acceptForSession", "decline"].includes(body.decision))
        throw new HttpError(400, "承認の指定が不正です。");
      const available = request.params.availableDecisions;
      if (Array.isArray(available) && !available.includes(body.decision))
        throw new HttpError(400, "この承認要求では選べない判断です。");
      result =
        request.method === "item/permissions/requestApproval"
          ? {
              permissions:
                body.decision !== "decline" ? request.params.permissions : {},
              scope: "turn",
            }
          : { decision: body.decision };
    }
    this.client.send({ id: request.rpcId, result });
    if (request.method.endsWith("requestUserInput")) {
      run.messages.push({
        id: `answer-${request.key}`,
        at: new Date().toISOString(),
        role: "human",
        text: request.params.questions
          .map(
            (question) =>
              `${question.question}\n回答: ${question.isSecret ? "（非表示）" : result.answers[question.id].answers.join("\n")}`,
          )
          .join("\n\n"),
      });
    }
    run.requests = run.requests.filter((r) => r !== request);
    if (!run.requests.length) run.status = "running";
    this.log(
      automatic ? "execution" : "decision",
      request.method.endsWith("requestUserInput")
        ? "人間が質問に回答しました。"
        : `${automatic ? "実行設定に従って応答" : "人間の実行判断"}: ${body.decision}`,
    );
    this.flush();
  }
  finish(status, error = null) {
    if (!this.active) return;
    // Keep before/after revisions as history, without invalidating decisions.
    const result = this.snapshot();
    Object.assign(this.active, {
      status,
      error,
      endedAt: new Date().toISOString(),
      requests: [],
      preview: "",
      resultFingerprint: result.fingerprint,
      resultHead: result.head,
      changedWorkspace: result.fingerprint !== this.active.fingerprint,
    });
    this.flush();
    this.active = null;
    this.emit("change");
    this.scheduleDrain();
  }
  async interrupt() {
    const run = this.active;
    if (!run) throw new HttpError(409, "実行中の作業はありません。");
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
    this.closed = true;
    clearImmediate(this.drainTimer);
    this.drainTimer = null;
    try {
      if (this.active) this.finish("interrupted", "アプリを終了しました。");
    } finally {
      this.active = null;
      this.client.close();
      clearTimeout(this.flushTimer);
    }
  }
}
