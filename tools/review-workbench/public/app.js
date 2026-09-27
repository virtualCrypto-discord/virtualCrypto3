const $ = (selector) => document.querySelector(selector);
const esc = (value) =>
  String(value ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
let state,
  detail,
  draft,
  selected = sessionStorage.getItem("review:selected"),
  category = "all",
  dirty = false,
  refreshing = false,
  refreshAgain = false,
  preparing = false;
const questionAnswers = new Map();
let selectionRequest = 0;
const runLabels = {
  starting: "接続中",
  running: "準備中",
  waiting: "回答・承認待ち",
  completed: "準備完了",
  failed: "失敗",
  interrupted: "中断",
};

function error(message) {
  $("#error").textContent = message;
  $("#error").hidden = !message;
}
function notice(message) {
  $("#notice").textContent = message;
  $("#notice").hidden = !message;
}
async function api(path, method = "GET", body) {
  const response = await fetch(path, {
    method,
    headers:
      method === "GET"
        ? {}
        : {
            "Content-Type": "application/json",
            "X-Review-Token": state?.token ?? "",
          },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
  return result;
}
function label(item) {
  if (item.stale) return "要再確認";
  if (state.active?.itemId === item.id) return runLabels[state.active.status];
  return state.statuses[item.displayStatus];
}
function badge(item) {
  return `<span class="badge ${item.stale ? "stale" : item.displayStatus}">${esc(label(item))}</span>`;
}
function settled(item) {
  return !item.stale && ["approved", "waived"].includes(item.displayStatus);
}
function renderOverview() {
  const done = state.items.filter(settled).length,
    stale = state.items.filter((i) => i.stale).length;
  $("#repository").innerHTML =
    `<strong>${esc(state.repository.branch)}</strong><code>${esc(state.repository.head.slice(0, 7))}</code>${state.repository.dirty ? '<span class="dirty">変更あり</span>' : ""}`;
  $("#connect").textContent = state.codex.authenticated
    ? "Codex 接続済み"
    : "Codex 接続確認";
  $("#progress").innerHTML =
    `<div><strong>${done}</strong><span> / ${state.items.length} 件の判断済み</span></div><progress value="${done}" max="${state.items.length}" aria-label="レビュー完了率"></progress><small>${state.items.filter((i) => i.displayStatus === "changes").length}件 要修正 · ${state.items.filter((i) => i.displayStatus === "blocked").length}件 保留 · ${stale}件 要再確認</small>`;
  $("#categories").innerHTML = [
    { id: "all", label: "すべての項目" },
    ...state.categories,
  ]
    .map((c) => {
      const items = state.items.filter(
        (i) => c.id === "all" || i.category === c.id,
      );
      return `<button data-category="${c.id}" class="category ${c.id === category ? "active" : ""}" aria-pressed="${c.id === category}"><span>${esc(c.label)}</span><span class="category-count">${items.filter(settled).length}/${items.length}</span></button>`;
    })
    .join("");
  const active = $("#active-run");
  active.hidden = !state.active;
  active.innerHTML = state.active
    ? `<span class="pulse"></span><span><strong>${esc(state.active.itemId)}</strong> ${esc(runLabels[state.active.status])}</span><button class="quiet" data-select="${esc(state.active.itemId)}">この項目を表示 →</button>`
    : "";
}
function renderList() {
  const query = $("#search").value.toLowerCase(),
    filter = $("#status-filter").value,
    critical = $("#critical-only").checked;
  const items = state.items.filter(
    (item) =>
      (category === "all" || item.category === category) &&
      (!critical || item.priority === "critical") &&
      (filter === "all" ||
        (filter === "open"
          ? !settled(item)
          : filter === "stale"
            ? item.stale
            : item.displayStatus === filter)) &&
      (!query ||
        [item.id, item.title, item.question, ...item.criteria, ...item.sources]
          .join(" ")
          .toLowerCase()
          .includes(query)),
  );
  $("#queue-title").textContent =
    category === "all"
      ? "すべての項目"
      : state.categories.find((c) => c.id === category)?.label;
  $("#queue-count").textContent = `${items.length}件`;
  $("#items").innerHTML =
    items
      .map(
        (item) =>
          `<button class="review-row ${item.id === selected ? "selected" : ""}" data-select="${item.id}" aria-pressed="${item.id === selected}"><div class="row-meta"><code>${item.id}</code>${item.priority === "critical" ? '<span class="priority-dot" title="重点項目">重点</span>' : ""}${badge(item)}</div><strong>${esc(item.title)}</strong><p>${esc(item.question)}</p></button>`,
      )
      .join("") ||
    '<div class="empty">該当する項目がありません。<br />検索条件を変更してください。</div>';
}
function initDraft() {
  draft = {
    version: detail.state.version,
    fingerprint: detail.fingerprint,
    status: detail.state.status,
    reviewer:
      detail.state.reviewer || localStorage.getItem("review:reviewer") || "",
    notes: detail.state.notes,
    evidence: detail.state.evidence,
    checks: detail.criteria.map((_, i) =>
      detail.stale ? false : (detail.state.checks[i] ?? false),
    ),
  };
  dirty = false;
}
function renderDetail() {
  if (!detail) return;
  const last = detail.state.runs.at(-1);
  $("#detail").innerHTML =
    `<div class="detail-heading"><div class="eyebrow">${esc(detail.id)} · ${esc(state.categories.find((c) => c.id === detail.category)?.label)}</div><h2>${esc(detail.title)}</h2><p class="question">${esc(detail.question)}</p><div id="detail-state">${badge(detail)}</div></div>
    <div id="stale-warning" class="warning" ${detail.stale ? "" : "hidden"}>対象コードまたは判断基準が変わっています。過去の判断は保持していますが、この版での再確認が必要です。</div>
    <section class="section"><h3><span class="step-number">1</span>判断基準</h3><div class="criteria">${detail.criteria.map((criterion, i) => `<label><input type="checkbox" data-criterion="${i}" ${draft.checks[i] ? "checked" : ""} /><span>${esc(criterion)}</span></label>`).join("")}</div><div class="sources">${detail.sources.map((path) => `<button class="source-link" data-source="${esc(path)}">↗ ${esc(path)}</button>`).join("")}</div></section>
    <section class="section preparation"><h3><span class="step-number">2</span>確認の準備<span class="codex-pill">Codex</span></h3><p>${esc(detail.preparation)}</p><label class="sr-only" for="instruction">LLMへの追加依頼</label><textarea id="instruction" rows="3" maxlength="10000" placeholder="これを確認したいので準備して。特に見たいケースや、利用するテスト環境を追記できます。"></textarea><div class="preparation-options"><label for="mode">実行範囲</label><select id="mode"><option value="read-only">読み取り・手順の準備</option><option value="workspace-write">ローカル準備（ファイル作成・テスト可）</option></select></div><div class="preparation-actions"><button id="prepare" class="primary">✦ 確認準備を依頼</button><button id="copy-prompt" class="secondary">依頼文をコピー</button></div><label class="continue-option"><input type="checkbox" id="continue-thread" ${last?.threadId ? "checked" : "disabled"} />前回の会話を続ける</label><p class="hint">既存のCodexログインを利用します。実行に確認が必要な場合、この画面に表示します。</p><div id="run"></div></section>
    <section class="section judgement"><h3><span class="step-number">3</span>人間の判断を記録</h3><div class="form-row"><label>進捗<select id="decision-status">${Object.entries(
      state.statuses,
    )
      .map(
        ([value, name]) =>
          `<option value="${value}" ${value === draft.status ? "selected" : ""}>${esc(name)}</option>`,
      )
      .join(
        "",
      )}</select></label><label>確認者<input id="reviewer" maxlength="200" value="${esc(draft.reviewer)}" placeholder="名前またはハンドル" /></label></div><label>判断メモ<textarea id="notes" rows="4" maxlength="20000" placeholder="何を確認し、なぜこの判断にしたか。保留・対象外の場合も理由を残します。">${esc(draft.notes)}</textarea></label><label>確認した根拠<textarea id="evidence" rows="3" maxlength="20000" placeholder="実行した手順と結果、ログの場所、画面を確認した環境など。秘密値は書かないでください。">${esc(draft.evidence)}</textarea></label><div class="save-row"><span id="save-state">${detail.state.updatedAt ? `保存済み · ${esc(new Date(detail.state.updatedAt).toLocaleString("ja-JP"))}` : "まだ判断は記録されていません"}</span><button id="save" class="primary">判断を保存</button></div><p class="hint">「確認済み」には全判断基準のチェック、確認者、判断メモ、根拠が必要です。LLMの準備完了だけでは確認済みになりません。</p>
    <details class="history"><summary>判断履歴 (${detail.state.history.length})</summary>${[
      ...detail.state.history,
    ]
      .reverse()
      .map(
        (h) =>
          `<article><strong>${esc(state.statuses[h.status])} · ${esc(h.reviewer || "未記入")}</strong><small>${esc(new Date(h.at).toLocaleString("ja-JP"))} · ${esc(h.head.slice(0, 7))}${h.dirty ? " + 未コミット変更" : ""}</small><p>${esc(h.notes)}</p><pre>${esc(h.evidence)}</pre></article>`,
      )
      .join("")}</details></section>`;
  renderRun();
}
function preserveFocus(fn) {
  const el = document.activeElement,
    id = el?.id,
    start = el?.selectionStart,
    end = el?.selectionEnd;
  fn();
  if (id) {
    const next = document.getElementById(id);
    if (next && next !== el) {
      next.focus({ preventScroll: true });
      try {
        if (start != null) next.setSelectionRange(start, end);
      } catch {
        /* Non-text input. */
      }
    }
  }
}
function renderRequest(request) {
  const p = request.params,
    key = request.key;
  if (request.method.endsWith("requestUserInput"))
    return `<form data-answer="${key}" class="approval"><h4>Codexからの質問</h4>${p.questions.map((q, index) => `<label>${esc(q.question)}${q.options?.length ? `<span class="answer-options">${q.options.map((o) => `<button type="button" class="secondary" data-answer-option="${esc(key)}" data-question="${esc(q.id)}" data-value="${esc(o.label)}">${esc(o.label)}<small>${esc(o.description)}</small></button>`).join("")}</span>` : ""}<input id="answer-${key}-${index}" name="${esc(q.id)}" data-question="${esc(q.id)}" data-request="${key}" value="${esc(questionAnswers.get(`${key}:${q.id}`) || "")}" maxlength="5000" required ${q.isSecret ? 'type="password"' : 'type="text"'} /></label>`).join("")}<button class="primary" type="submit">回答を送る</button></form>`;
  const available = p.availableDecisions;
  const canAccept = !Array.isArray(available) || available.includes("accept");
  const preview =
    p.command || request.changes || JSON.stringify(p.permissions || p, null, 2);
  const context = JSON.stringify(
    {
      cwd: p.cwd,
      grantRoot: p.grantRoot,
      additionalPermissions: p.additionalPermissions,
      network: p.networkApprovalContext,
    },
    null,
    2,
  );
  return `<div class="approval"><h4>${request.method.includes("permissions") ? "追加権限の確認" : request.method.includes("fileChange") ? "ファイル変更の確認" : "コマンド実行の確認"}</h4><p>${esc(p.reason || "実行する内容を確認してください。")}</p><pre>${esc(preview)}</pre>${context !== "{}" ? `<details><summary>実行先・権限の詳細</summary><pre>${esc(context)}</pre></details>` : ""}<div class="button-row">${canAccept ? `<button class="primary" data-approval="${key}" data-decision="accept">今回のみ許可</button>` : ""}<button class="secondary" data-approval="${key}" data-decision="decline">許可しない</button></div></div>`;
}
function renderRun() {
  if (!detail || !$("#run")) return;
  const run = detail.state.runs.at(-1),
    active = state.active?.itemId === selected;
  $("#prepare").disabled = preparing || Boolean(state.active);
  preserveFocus(() => {
    $("#run").innerHTML = run
      ? `<div class="run-header"><strong>${esc(runLabels[run.status] || run.status)}</strong><span>${esc(new Date(run.startedAt).toLocaleString("ja-JP"))}</span>${active ? `<button class="quiet danger" id="interrupt" data-run-id="${run.id}">中断</button>` : ""}</div>${detail.preparationStale ? '<p class="warning">この準備結果は現在のコードより古い可能性があります。根拠を再確認してください。</p>' : ""}${run.error ? `<p class="warning">${esc(run.error)}</p>` : ""}${(run.requests || []).map(renderRequest).join("")}${run.preview ? `<pre class="report live-report">${esc(run.preview)}</pre>` : ""}${run.report ? `<div class="report-heading">準備結果</div><pre class="report">${esc(run.report)}</pre>` : ""}${run.diff ? `<details><summary>準備中のファイル変更</summary><pre class="activity">${esc(run.diff)}</pre></details>` : ""}<details class="activity-details" ${run.status === "running" && !run.report ? "open" : ""}><summary>実行の記録 (${run.events.length})</summary><div class="activity">${run.events.map((event) => `<article><small>${esc(new Date(event.at).toLocaleTimeString("ja-JP"))} · ${esc(event.kind)}</small><pre>${esc(event.detail)}</pre></article>`).join("") || "記録を待っています。"}</div></details>${
          detail.state.runs.length > 1
            ? `<details><summary>過去の準備 (${detail.state.runs.length - 1})</summary>${detail.state.runs
                .slice(0, -1)
                .reverse()
                .map(
                  (past) =>
                    `<article class="past-run"><strong>${esc(new Date(past.startedAt).toLocaleString("ja-JP"))} · ${esc(runLabels[past.status])}</strong><pre class="report">${esc(past.report || past.error || "結果なし")}</pre></article>`,
                )
                .join("")}</details>`
            : ""
        }`
      : '<div class="run-empty">依頼すると、ここに準備状況と確認手順が表示されます。</div>';
  });
}
async function select(id) {
  if (
    dirty &&
    !confirm("未保存の判断メモがあります。項目を移動して破棄しますか？")
  )
    return;
  const request = ++selectionRequest;
  const incoming = await api(`/api/items/${id}`);
  if (request !== selectionRequest) return;
  selected = id;
  sessionStorage.setItem("review:selected", id);
  detail = incoming;
  initDraft();
  renderList();
  renderDetail();
  error("");
  notice("");
}
async function refresh() {
  if (refreshing) {
    refreshAgain = true;
    return;
  }
  refreshing = true;
  try {
    state = await api("/api/state");
    renderOverview();
    renderList();
    if (!detail)
      await select(
        state.items.some((i) => i.id === selected)
          ? selected
          : state.items[0].id,
      );
    else {
      const id = selected,
        incoming = await api(`/api/items/${id}`);
      if (selected === id) {
        const humanChanged = incoming.state.version !== detail.state.version;
        detail = incoming;
        if (humanChanged && !dirty) {
          initDraft();
          renderDetail();
        } else {
          $("#stale-warning").hidden =
            !detail.stale && draft.fingerprint === detail.fingerprint;
          $("#detail-state").innerHTML = badge(detail);
          renderRun();
        }
      }
    }
  } catch (e) {
    error(e.message);
  } finally {
    refreshing = false;
    if (refreshAgain) {
      refreshAgain = false;
      setTimeout(refresh, 250);
    }
  }
}
async function save() {
  const id = selected;
  const updated = await api(`/api/items/${id}`, "PATCH", draft);
  localStorage.setItem("review:reviewer", draft.reviewer);
  if (selected === id) {
    detail = updated;
    initDraft();
    renderDetail();
  }
  notice("判断を保存しました。");
  await refresh();
}
function changed() {
  dirty = true;
  $("#save-state").textContent = "未保存の変更があります";
}
document.addEventListener("input", (event) => {
  const el = event.target;
  if (el.id === "search") renderList();
  if (el.matches("[data-criterion]")) {
    draft.checks[Number(el.dataset.criterion)] = el.checked;
    changed();
  }
  if (el.id === "reviewer" || el.id === "notes" || el.id === "evidence") {
    draft[el.id] = el.value;
    changed();
  }
  if (el.id === "decision-status") {
    draft.status = el.value;
    changed();
  }
  if (el.dataset.request)
    questionAnswers.set(
      `${el.dataset.request}:${el.dataset.question}`,
      el.value,
    );
});
for (const id of ["status-filter", "critical-only"])
  $(`#${id}`).addEventListener("change", renderList);
document.addEventListener("click", async (event) => {
  const button = event.target.closest("button");
  if (!button) return;
  try {
    error("");
    if (button.dataset.category) {
      category = button.dataset.category;
      renderOverview();
      renderList();
    } else if (button.dataset.select) await select(button.dataset.select);
    else if (button.dataset.close)
      document.getElementById(button.dataset.close).close();
    else if (button.dataset.source) {
      const source = await api(
        `/api/source?path=${encodeURIComponent(button.dataset.source)}`,
      );
      $("#source-title").textContent = source.path;
      $("#source-content").textContent =
        source.content
          .split("\n")
          .map((line, i) => `${String(i + 1).padStart(4)}  ${line}`)
          .join("\n") + (source.truncated ? "\n表示は途中までです。" : "");
      $("#source-dialog").showModal();
    } else if (button.id === "add-item") $("#add-dialog").showModal();
    else if (button.id === "connect") {
      button.disabled = true;
      try {
        const info = await api("/api/connect", "POST", {});
        notice(
          info.authenticated
            ? "Codexに接続しました。既存のログインを使用できます。"
            : "Codexへのログインが必要です。ターミナルで codex login を実行してください。",
        );
        await refresh();
      } finally {
        button.disabled = false;
      }
    } else if (button.id === "save") await save();
    else if (button.id === "copy-prompt") {
      const result = await api(`/api/items/${selected}/prompt`);
      const instruction = $("#instruction").value.trim();
      await navigator.clipboard.writeText(
        result.prompt + (instruction ? `\n追加依頼: ${instruction}` : ""),
      );
      notice("依頼文をコピーしました。別のハーネスにも貼り付けられます。");
    } else if (button.id === "prepare") {
      const itemId = selected,
        fingerprint = detail.fingerprint,
        instruction = $("#instruction").value,
        mode = $("#mode").value,
        continuation = $("#continue-thread").checked;
      if (dirty) await save();
      preparing = true;
      renderRun();
      try {
        await api(`/api/items/${itemId}/prepare`, "POST", {
          instruction,
          mode,
          continue: continuation,
          fingerprint,
        });
        await refresh();
      } finally {
        preparing = false;
        renderRun();
      }
    } else if (button.id === "interrupt") {
      await api("/api/interrupt", "POST", { runId: button.dataset.runId });
      notice("中断を依頼しました。実行記録を確認してください。");
      await refresh();
    } else if (button.dataset.approval) {
      await api(`/api/requests/${button.dataset.approval}`, "POST", {
        decision: button.dataset.decision,
      });
      await refresh();
    } else if (button.dataset.answerOption) {
      const key = `${button.dataset.answerOption}:${button.dataset.question}`;
      questionAnswers.set(key, button.dataset.value);
      renderRun();
    }
  } catch (e) {
    error(e.message);
  }
});
document.addEventListener("submit", async (event) => {
  const form = event.target;
  event.preventDefault();
  error("");
  try {
    if (form.id === "add-form") {
      const item = await api(
        "/api/items",
        "POST",
        Object.fromEntries(new FormData(form)),
      );
      $("#add-dialog").close();
      form.reset();
      category = "all";
      await refresh();
      await select(item.id);
    } else if (form.dataset.answer) {
      await api(`/api/requests/${form.dataset.answer}`, "POST", {
        answers: Object.fromEntries(new FormData(form)),
      });
      await refresh();
    }
  } catch (e) {
    error(e.message);
  }
});
window.addEventListener("beforeunload", (event) => {
  if (dirty) {
    event.preventDefault();
    event.returnValue = "";
  }
});
await refresh();
const events = new EventSource("/api/events");
let refreshTimer;
events.addEventListener("change", () => {
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(refresh, 150);
});
events.onopen = () => {
  $("#live").textContent = "ローカル接続";
  refresh();
};
events.onerror = () => {
  $("#live").textContent = "再接続待ち";
};
setInterval(refresh, 15000);
