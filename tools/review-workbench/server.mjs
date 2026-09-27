import { createServer } from "node:http";
import { randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  readFileSync,
  realpathSync,
  mkdirSync,
  writeFileSync,
  unlinkSync,
} from "node:fs";
import { dirname, join, resolve, sep } from "node:path";
import { homedir } from "node:os";
import { fileURLToPath, pathToFileURL } from "node:url";
import { categories, catalogVersion } from "./lib/catalog.mjs";
import {
  Store,
  HttpError,
  exportMarkdown,
  hash,
  object,
  text,
  statuses,
  workspaceSnapshot,
  workspaceDiff,
} from "./lib/store.mjs";
import { CodexClient, ReviewManager } from "./lib/codex.mjs";
import { reviewPrompt } from "./lib/prompt.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const publicFiles = {
  "/": ["index.html", "text/html"],
  "/app.js": ["app.js", "text/javascript"],
  "/style.css": ["style.css", "text/css"],
};

function lock(directory) {
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  const path = join(directory, "instance.lock");
  if (existsSync(path)) {
    const pid = Number(readFileSync(path, "utf8"));
    if (!Number.isSafeInteger(pid) || pid < 1)
      throw new Error(
        "保存先のinstance.lockが不正です。起動済みアプリがないか確認してください。",
      );
    try {
      process.kill(pid, 0);
      throw new Error("同じ保存先のレビューアプリがすでに起動しています。");
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
    unlinkSync(path);
  }
  writeFileSync(path, String(process.pid), { flag: "wx", mode: 0o600 });
  return () => {
    if (existsSync(path) && readFileSync(path, "utf8") === String(process.pid))
      unlinkSync(path);
  };
}

async function bodyOf(req) {
  if (!/^application\/json(?:;|$)/i.test(req.headers["content-type"] ?? ""))
    throw new HttpError(415, "application/jsonが必要です。");
  if (Number(req.headers["content-length"] ?? 0) > 256 * 1024)
    throw new HttpError(413, "入力が大きすぎます。");
  const raw = await new Promise((resolveBody, reject) => {
    const chunks = [];
    let length = 0,
      failed = false;
    req.on("data", (chunk) => {
      if (failed) return;
      length += chunk.length;
      if (length > 256 * 1024) {
        failed = true;
        chunks.length = 0;
        reject(new HttpError(413, "入力が大きすぎます。"));
      } else chunks.push(chunk);
    });
    req.on("end", () => {
      if (!failed) resolveBody(Buffer.concat(chunks).toString("utf8"));
    });
    req.on("error", reject);
  });
  try {
    return object(JSON.parse(raw));
  } catch (error) {
    if (error instanceof HttpError) throw error;
    throw new HttpError(400, "JSONを読み取れません。");
  }
}

export function createWorkbench({
  root = process.env.REVIEW_REPO_ROOT || resolve(here, "../.."),
  dataDirectory,
  client,
} = {}) {
  root = realpathSync(root);
  execFileSync("git", ["rev-parse", "--show-toplevel"], {
    cwd: root,
    stdio: "pipe",
  });
  dataDirectory ??=
    process.env.REVIEW_DATA_DIR ||
    join(
      process.env.XDG_STATE_HOME || join(homedir(), ".local", "state"),
      "virtualcrypto-review",
      hash(root).slice(0, 20),
    );
  // Keep review records outside the repository and Git history.
  const unlock = lock(dataDirectory);
  let store;
  try {
    store = new Store(dataDirectory);
  } catch (error) {
    unlock();
    throw error;
  }
  let cached,
    cachedAt = 0;
  const snapshot = (fresh = false) => {
    if (fresh || !cached || Date.now() - cachedAt > 1000) {
      cached = workspaceSnapshot(root);
      cachedAt = Date.now();
    }
    return cached;
  };
  const token = randomBytes(32).toString("hex");
  client ??= new CodexClient({ cwd: root });
  const manager = new ReviewManager({
    client,
    store,
    root,
    snapshot: () => snapshot(true),
  });
  const streams = new Set();
  const broadcast = () => {
    for (const stream of streams) stream.write("event: change\ndata: {}\n\n");
  };
  manager.on("change", broadcast);
  const heartbeat = setInterval(() => {
    for (const stream of streams) stream.write(": keepalive\n\n");
  }, 20000);
  heartbeat.unref();
  const summarize = (item) => ({
    ...item,
    state: {
      ...item.state,
      history: [],
      runs: item.state.runs.map((r) => ({
        id: r.id,
        status: r.status,
        startedAt: r.startedAt,
        threadId: r.threadId,
      })),
    },
  });

  const server = createServer(async (req, res) => {
    res.setHeader("Cache-Control", "no-store");
    res.setHeader("X-Content-Type-Options", "nosniff");
    res.setHeader("Referrer-Policy", "no-referrer");
    res.setHeader(
      "Content-Security-Policy",
      "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
    );
    const json = (value, status = 200) => {
      res.writeHead(status, {
        "Content-Type": "application/json; charset=utf-8",
      });
      res.end(JSON.stringify(value));
    };
    try {
      const port = server.address()?.port;
      const hosts = [`127.0.0.1:${port}`, `localhost:${port}`];
      if (!hosts.includes(req.headers.host))
        throw new HttpError(403, "ローカルのURLからアクセスしてください。");
      const origin = `http://${req.headers.host}`;
      if (
        (req.headers.origin && req.headers.origin !== origin) ||
        req.headers["sec-fetch-site"] === "cross-site"
      )
        throw new HttpError(403, "別のサイトからのアクセスは受け付けません。");
      const url = new URL(req.url, origin);
      if (req.method === "GET" && publicFiles[url.pathname]) {
        const [path, type] = publicFiles[url.pathname];
        res.writeHead(200, { "Content-Type": `${type}; charset=utf-8` });
        res.end(readFileSync(join(here, "public", path)));
        return;
      }
      if (!url.pathname.startsWith("/api/"))
        throw new HttpError(404, "見つかりません。");
      if (!["GET", "POST", "PATCH"].includes(req.method))
        throw new HttpError(405, "この操作には対応していません。");
      if (req.method !== "GET" && req.headers["x-review-token"] !== token)
        throw new HttpError(403, "画面を再読み込みしてください。");
      const body = req.method === "GET" ? null : await bodyOf(req);
      if (req.method === "GET" && url.pathname === "/api/events") {
        res.writeHead(200, {
          "Content-Type": "text/event-stream",
          Connection: "keep-alive",
        });
        res.write(": connected\n\n");
        streams.add(res);
        req.on("close", () => streams.delete(res));
        return;
      }
      if (req.method === "GET" && url.pathname === "/api/state") {
        const repository = snapshot();
        json({
          token,
          repository,
          categories: [...categories, { id: "custom", label: "追加項目" }],
          statuses,
          catalogVersion,
          items: store
            .items()
            .map((item) => summarize(store.present(item, repository))),
          codex: client.info,
          active: manager.active
            ? {
                id: manager.active.id,
                itemId: manager.active.itemId,
                status: manager.active.status,
              }
            : null,
        });
        return;
      }
      if (req.method === "GET" && url.pathname === "/api/export") {
        const repository = snapshot(true),
          items = store.items().map((item) => store.present(item, repository));
        const format =
          url.searchParams.get("format") === "json" ? "json" : "md";
        res.setHeader(
          "Content-Disposition",
          `attachment; filename="virtualcrypto-review.${format}"`,
        );
        res.writeHead(200, {
          "Content-Type":
            format === "json"
              ? "application/json; charset=utf-8"
              : "text/markdown; charset=utf-8",
        });
        res.end(
          format === "json"
            ? JSON.stringify(
                {
                  schema: 1,
                  exportedAt: new Date().toISOString(),
                  repository,
                  items,
                },
                null,
                2,
              )
            : exportMarkdown(items, repository),
        );
        return;
      }
      if (req.method === "GET" && url.pathname === "/api/source") {
        const path = url.searchParams.get("path");
        if (!store.items().some((item) => item.sources.includes(path)))
          throw new HttpError(404, "この参照先は登録されていません。");
        const actual = realpathSync(join(root, path));
        if (!actual.startsWith(root + sep))
          throw new HttpError(403, "リポジトリ外は表示できません。");
        const content = readFileSync(actual, "utf8");
        json({
          path,
          content: content.slice(0, 200000),
          truncated: content.length > 200000,
        });
        return;
      }
      if (req.method === "GET" && url.pathname === "/api/diff") {
        json({ diff: workspaceDiff(root), repository: snapshot(true) });
        return;
      }
      if (req.method === "POST" && url.pathname === "/api/connect") {
        json(await client.connect());
        return;
      }
      if (req.method === "POST" && url.pathname === "/api/interrupt") {
        if (manager.active?.id !== body.runId)
          throw new HttpError(
            409,
            "実行中の作業が変わりました。画面を更新してください。",
          );
        await manager.interrupt();
        json({ ok: true });
        return;
      }
      if (req.method === "POST" && url.pathname === "/api/items") {
        const item = store.add(body);
        broadcast();
        json(item, 201);
        return;
      }
      const itemMatch = url.pathname.match(
        /^\/api\/items\/([A-Z0-9-]+)(?:\/(messages|prompt))?$/,
      );
      if (itemMatch) {
        const [, id, action] = itemMatch,
          item = store.item(id);
        if (req.method === "GET" && !action) {
          json(store.present(item, snapshot()));
          return;
        }
        if (req.method === "POST" && action === "prompt") {
          json({
            prompt: reviewPrompt(
              item,
              store.record(id),
              snapshot(),
              text(body.instruction ?? "", "メッセージ", 10000),
              body.mode === "read-only" ? "read-only" : "workspace-write",
              text(body.context ?? "", "編集中のメモ", 45000),
            ),
          });
          return;
        }
        if (req.method === "PATCH" && !action) {
          json(store.update(id, body, snapshot(true)));
          broadcast();
          return;
        }
        if (req.method === "POST" && action === "messages") {
          const run = await manager.start(id, body);
          json({ id: run.id, status: run.status, error: run.error }, 202);
          return;
        }
      }
      const decisionMatch = url.pathname.match(
        /^\/api\/requests\/([a-f0-9-]+)$/,
      );
      if (req.method === "POST" && decisionMatch) {
        manager.answer(decisionMatch[1], body);
        json({ ok: true });
        return;
      }
      throw new HttpError(404, "見つかりません。");
    } catch (error) {
      if (!res.headersSent)
        json(
          {
            error:
              error.code === "ENOENT"
                ? "参照先が見つかりません。"
                : error.message,
          },
          error.status ?? 500,
        );
      else res.end();
    }
  });
  server.headersTimeout = 15000;
  server.requestTimeout = 30000;
  let closed = false;
  return {
    server,
    store,
    manager,
    client,
    dataDirectory,
    listen(port = 4317) {
      return new Promise((resolveListen, reject) => {
        server.once("error", reject);
        server.listen(port, "127.0.0.1", () => {
          server.off("error", reject);
          resolveListen(server.address());
        });
      });
    },
    async close() {
      if (closed) return;
      closed = true;
      clearInterval(heartbeat);
      try {
        manager.close();
      } finally {
        for (const stream of streams) stream.end();
        server.closeAllConnections();
        await new Promise((resolveClose) => server.close(resolveClose));
        unlock();
      }
    },
  };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  let app;
  try {
    const port = Number(process.env.REVIEW_PORT || 4317);
    if (!Number.isInteger(port) || port < 1 || port > 65535)
      throw new Error("REVIEW_PORTが不正です。");
    app = createWorkbench();
    await app.listen(port);
    console.log(`レビュー作業室: http://127.0.0.1:${port}`);
    console.log(
      "終了: Ctrl+C。Codexはメッセージの送信または接続確認で起動します。",
    );
    for (const signal of ["SIGINT", "SIGTERM"])
      process.once(signal, async () => {
        await app.close();
        process.exit(0);
      });
  } catch (error) {
    console.error(error.message);
    if (app) await app.close();
    process.exitCode = 1;
  }
}
