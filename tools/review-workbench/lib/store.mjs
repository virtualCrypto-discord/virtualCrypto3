import { createHash, randomUUID } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readlinkSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import { basename, join } from "node:path";
import { catalog } from "./catalog.mjs";

export class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}
export const statuses = {
  todo: "未着手",
  ready: "確認待ち",
  reviewing: "確認中",
  approved: "確認済み",
  changes: "要修正",
  blocked: "保留",
  waived: "対象外",
};
export const hash = (value) => createHash("sha256").update(value).digest("hex");
export const itemHash = (item) => hash(JSON.stringify(item));
export function text(value, name, max = 20000) {
  if (typeof value !== "string" || value.length > max)
    throw new HttpError(400, `${name}の形式または長さが不正です。`);
  return value;
}
export function object(value) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new HttpError(400, "JSONオブジェクトが必要です。");
  return value;
}

// Include unstaged, staged, and untracked source edits. Ignore Git-ignored local
// artifacts and hash symlink targets as strings, without following them.
export function workspaceSnapshot(root) {
  const git = (...args) =>
    execFileSync("git", args, {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
    });
  const head = git("rev-parse", "HEAD").trim();
  const branch = git("branch", "--show-current").trim() || "detached HEAD";
  const files = [
    ...new Set(
      git("ls-files", "-z", "--cached", "--others", "--exclude-standard")
        .split("\0")
        .filter(Boolean),
    ),
  ].sort();
  const digest = createHash("sha256").update(head);
  for (const file of files) {
    digest.update(`\0${file}\0`);
    try {
      const path = join(root, file),
        stat = lstatSync(path);
      digest.update(String(stat.mode));
      if (stat.isSymbolicLink()) digest.update(readlinkSync(path));
      else if (stat.isFile()) digest.update(readFileSync(path));
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      digest.update("deleted");
    }
  }
  return {
    name: basename(root),
    head,
    branch,
    dirty: Boolean(git("status", "--porcelain").trim()),
    fingerprint: digest.digest("hex"),
  };
}

export function workspaceDiff(root) {
  const git = (...args) => {
    try {
      return execFileSync("git", args, {
        cwd: root,
        encoding: "utf8",
        maxBuffer: 2 * 1024 * 1024,
      });
    } catch (error) {
      // --no-index uses exit status 1 for an ordinary difference.
      if (error.status === 1 || error.code === "ENOBUFS")
        return (
          String(error.stdout ?? "") +
          (error.code === "ENOBUFS" ? "\n…（長い差分を省略）" : "")
        );
      throw error;
    }
  };
  const parts = [
    git("diff", "--no-ext-diff", "--no-textconv", "HEAD", "--", "."),
  ];
  let length = parts[0].length;
  for (const file of git("ls-files", "--others", "--exclude-standard", "-z")
    .split("\0")
    .filter(Boolean)) {
    if (length >= 200000) break;
    const stat = lstatSync(join(root, file));
    const part =
      stat.isSymbolicLink() || !stat.isFile() || stat.size > 200000
        ? `\n未追跡: ${file}（内容の表示対象外）\n`
        : git(
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            "/dev/null",
            file,
          );
    parts.push(part);
    length += part.length;
  }
  const diff = parts.join("\n");
  return (
    diff.slice(0, 200000) + (length >= 200000 ? "\n…（長い差分を省略）" : "")
  );
}

export class Store {
  constructor(directory) {
    this.directory = directory;
    this.path = join(directory, "state.json");
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    this.data = existsSync(this.path)
      ? JSON.parse(readFileSync(this.path, "utf8"))
      : { schema: 1, customItems: [], records: {} };
    if (
      this.data.schema !== 1 ||
      !Array.isArray(this.data.customItems) ||
      !this.data.records ||
      typeof this.data.records !== "object"
    ) {
      throw new Error("保存データの形式が不正です。上書きせず停止します。");
    }
    // A server restart cannot silently report an unfinished preparation as ready.
    let recovered = false;
    for (const record of Object.values(this.data.records)) {
      for (const run of record.runs ?? []) {
        if (["starting", "running", "waiting"].includes(run.status)) {
          Object.assign(run, {
            status: "interrupted",
            endedAt: new Date().toISOString(),
            error:
              "アプリが終了したため作業を中断しました。会話から続きを依頼してください。",
          });
          run.requests = [];
          recovered = true;
        }
      }
    }
    if (recovered) this.persist(this.data);
  }
  persist(next) {
    const temporary = `${this.path}.${randomUUID()}.tmp`;
    writeFileSync(temporary, JSON.stringify(next, null, 2) + "\n", {
      mode: 0o600,
      flag: "wx",
    });
    renameSync(temporary, this.path);
    this.data = next;
  }
  items() {
    return [...catalog, ...this.data.customItems];
  }
  item(id) {
    const item = this.items().find((entry) => entry.id === id);
    if (!item) throw new HttpError(404, "レビュー項目がありません。");
    return item;
  }
  record(id) {
    this.item(id);
    return structuredClone(
      this.data.records[id] ?? {
        version: 0,
        status: "todo",
        reviewer: "",
        notes: "",
        evidence: "",
        checks: [],
        reviewedFingerprint: null,
        reviewedItemHash: null,
        updatedAt: null,
        history: [],
        runs: [],
      },
    );
  }
  present(item, repository) {
    const state = this.record(item.id);
    const latest = state.runs.at(-1);
    const displayStatus =
      ["todo", "changes", "reviewing"].includes(state.status) &&
      latest?.status === "completed" &&
      (!state.updatedAt || latest.endedAt > state.updatedAt)
        ? "ready"
        : state.status;
    return {
      ...item,
      state,
      displayStatus,
      fingerprint: repository.fingerprint,
    };
  }
  update(id, body, repository) {
    object(body);
    const item = this.item(id),
      state = this.record(id);
    if (body.version !== state.version)
      throw new HttpError(
        409,
        "別の画面で更新されています。再読み込みして内容を確認してください。",
      );
    if (!Object.hasOwn(statuses, body.status))
      throw new HttpError(400, "進捗の指定が不正です。");
    const reviewer = text(body.reviewer, "確認者", 200).trim();
    const notes = text(body.notes, "判断メモ").trim();
    const evidence = text(body.evidence, "確認した根拠").trim();
    if (
      !Array.isArray(body.checks) ||
      body.checks.length !== item.criteria.length ||
      body.checks.some((v) => typeof v !== "boolean")
    ) {
      throw new HttpError(400, "判断基準の確認状態が不正です。");
    }
    if (body.status === "approved" && !body.checks.every(Boolean)) {
      throw new HttpError(
        400,
        "確認済みにするには、判断基準をチェックしてください。",
      );
    }
    const now = new Date().toISOString();
    const entry = {
      at: now,
      status: body.status,
      reviewer,
      notes,
      evidence,
      checks: body.checks,
      fingerprint: repository.fingerprint,
      head: repository.head,
      dirty: repository.dirty,
      itemHash: itemHash(item),
    };
    Object.assign(state, {
      status: body.status,
      reviewer,
      notes,
      evidence,
      checks: body.checks,
      version: state.version + 1,
      updatedAt: now,
      reviewedFingerprint: repository.fingerprint,
      reviewedItemHash: itemHash(item),
      history: [...state.history, entry],
    });
    const next = structuredClone(this.data);
    next.records[id] = state;
    this.persist(next);
    return this.present(item, repository);
  }
  add(body) {
    object(body);
    const title = text(body.title, "タイトル", 200).trim();
    const question = text(body.question, "判断したいこと", 5000).trim();
    const criteria = text(body.criteria, "判断基準", 5000)
      .split("\n")
      .map((v) => v.trim())
      .filter(Boolean);
    if (!title || !question || !criteria.length || criteria.length > 20)
      throw new HttpError(
        400,
        "タイトル・判断したいこと・1〜20件の判断基準を記入してください。",
      );
    const item = {
      id: `CUSTOM-${randomUUID().slice(0, 8).toUpperCase()}`,
      category: "custom",
      title,
      question,
      criteria,
      preparation:
        "追加項目の背景を確認し、根拠・再現手順・未決事項を用意する。",
      sources: [],
      priority: "normal",
    };
    const next = structuredClone(this.data);
    next.customItems.push(item);
    this.persist(next);
    return item;
  }
  saveRun(id, run) {
    const next = structuredClone(this.data),
      state = this.record(id);
    const position = state.runs.findIndex((entry) => entry.id === run.id);
    if (position < 0) state.runs.push(structuredClone(run));
    else state.runs[position] = structuredClone(run);
    next.records[id] = state;
    this.persist(next);
  }
}

export function exportMarkdown(items, repository) {
  const lines = [
    "# virtualCrypto レビュー記録",
    "",
    `対象: ${repository.head} (${repository.branch}${repository.dirty ? ", 未コミット変更あり" : ""})`,
    `出力: ${new Date().toISOString()}`,
    "",
    "進捗とチェックは人間の判断記録です。コード更新によって自動的に差し戻しません。",
    "",
  ];
  for (const item of items) {
    const s = item.state;
    lines.push(
      `## ${item.id} ${item.title}`,
      "",
      `状態: ${statuses[item.displayStatus]}`,
      `確認者: ${s.reviewer || "未記入"}`,
      "",
      item.question,
      "",
      ...item.criteria.map(
        (criterion, i) => `- [${s.checks[i] ? "x" : " "}] ${criterion}`,
      ),
      "",
      "### 判断メモ",
      "",
      s.notes || "未記入",
      "",
      "### 確認した根拠",
      "",
      s.evidence || "未記入",
      "",
      "### 参照先",
      "",
      ...item.sources.map((source) => `- ${source}`),
      "",
    );
    for (const run of s.runs) {
      lines.push(
        "### 会話・作業記録",
        "",
        `日時: ${run.startedAt} / 状態: ${run.status} / 実行範囲: ${run.mode}`,
        "",
        "人間:",
        run.instruction || "確認の準備を依頼",
        "",
        "応答:",
        run.messages?.length
          ? run.messages
              .map(
                (message) =>
                  `${message.role === "human" ? "人間" : "Codex"}:\n${message.text}`,
              )
              .join("\n\n")
          : run.report || run.error || "応答なし",
        ...(run.diff ? ["", "変更差分:", run.diff] : []),
        "",
      );
    }
  }
  return lines.join("\n");
}
