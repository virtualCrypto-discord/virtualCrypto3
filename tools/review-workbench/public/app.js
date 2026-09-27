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
  sending = false;
const questionAnswers = new Map();
const composers = new Map();
const humanDrafts = new Map();
function composer(id = selected) {
  if (!composers.has(id))
    composers.set(id, {
      instruction: "",
      mode: "workspace-write",
      newConversation: false,
    });
  return composers.get(id);
}
function draftContext() {
  return dirty
    ? `進捗の編集中の値: ${draft.status}\n判断メモ: ${draft.notes}\n根拠: ${draft.evidence}`
    : "";
}
let selectionRequest = 0;
const runLabels = {
  starting: "接続中",
  running: "作業中",
  waiting: "回答・承認待ち",
  completed: "応答完了",
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
  const input = composer();
  $("#detail").innerHTML =
    `<div class="detail-heading"><div class="eyebrow">${esc(detail.id)} · ${esc(state.categories.find((c) => c.id === detail.category)?.label)}</div><h2>${esc(detail.title)}</h2><p class="question">${esc(detail.question)}</p><div id="detail-state">${badge(detail)}</div></div>
    <div id="stale-warning" class="warning" ${detail.stale ? "" : "hidden"}>対象コードまたは判断基準が変わっています。この版で再確認して判断を記録してください。<button id="recheck" class="secondary">この版で再確認を始める</button></div>
    <section class="section"><h3><span class="step-number">1</span>判断基準</h3><div class="criteria">${detail.criteria.map((criterion, i) => `<label><input type="checkbox" data-criterion="${i}" ${draft.checks[i] ? "checked" : ""} /><span>${esc(criterion)}</span></label>`).join("")}</div><div class="sources">${detail.sources.map((path) => `<button class="source-link" data-source="${esc(path)}">↗ ${esc(path)}</button>`).join("")}</div></section>
    <section class="section conversation-section"><h3><span class="step-number">2</span>Codexと進める<span class="codex-pill">会話・修正・検証</span></h3><details class="preparation-hint"><summary>この項目の確認に役立つ準備案</summary><p>${esc(detail.preparation)}</p></details><div id="conversation" class="conversation" tabindex="0" aria-label="この項目の会話履歴"></div><div id="run"></div>
    <div class="composer"><label for="instruction">メッセージ</label><textarea id="instruction" rows="4" maxlength="10000" placeholder="この項目を確認したいので準備して。／この挙動をこう修正して。／修正後のケースをもう一度確認して。">${esc(input.instruction)}</textarea><div class="message-suggestions"><button class="quiet" data-message="この項目を確認したいので準備してください。">確認の準備</button><button class="quiet" data-message="指摘した点を修正して、必要なテストも実行してください。">修正を依頼</button><button class="quiet" data-message="現在の修正を踏まえて、判断基準に沿ってもう一度確認してください。">修正後を再確認</button></div><div class="preparation-options"><label for="mode">実行範囲</label><select id="mode"><option value="workspace-write" ${input.mode === "workspace-write" ? "selected" : ""}>修正・コマンド実行可</option><option value="read-only" ${input.mode === "read-only" ? "selected" : ""}>読み取りのみ</option></select></div><label class="continue-option"><input type="checkbox" id="new-conversation" ${input.newConversation ? "checked" : ""} />新しい会話で始める（以前の記録は残ります）</label><div class="preparation-actions"><button id="send" class="primary">送信</button><button id="copy-prompt" class="secondary">依頼文をコピー</button><button id="workspace-diff" class="quiet">現在の変更を見る</button></div><p class="hint">この項目の会話を続けます。相談・調査・実装の修正・テストを依頼できます。Ctrl / ⌘ + Enterで送信。</p></div></section>
    <section class="section judgement"><h3><span class="step-number">3</span>人間の判断を記録</h3><div class="form-row"><label>進捗<select id="decision-status">${Object.entries(
      state.statuses,
    )
      .map(
        ([value, name]) =>
          `<option value="${value}" ${value === draft.status ? "selected" : ""}>${esc(name)}</option>`,
      )
      .join(
        "",
      )}</select></label><label>確認者<input id="reviewer" maxlength="200" value="${esc(draft.reviewer)}" placeholder="名前またはハンドル" /></label></div><label>判断メモ<textarea id="notes" rows="4" maxlength="20000" placeholder="何を確認し、なぜこの判断にしたか。保留・対象外の場合も理由を残します。">${esc(draft.notes)}</textarea></label><label>確認した根拠<textarea id="evidence" rows="3" maxlength="20000" placeholder="実行した手順と結果、ログの場所、画面を確認した環境など。秘密値は書かないでください。">${esc(draft.evidence)}</textarea></label><div class="save-row"><span id="save-state">${detail.state.updatedAt ? `保存済み · ${esc(new Date(detail.state.updatedAt).toLocaleString("ja-JP"))}` : "まだ判断は記録されていません"}</span><button id="save" class="primary">判断を保存</button></div><p class="hint">「確認済み」には全判断基準のチェック、確認者、判断メモ、根拠が必要です。修正やテストの完了後、人間が内容を確認して記録します。</p>
    <details class="history"><summary>判断履歴 (${detail.state.history.length})</summary>${[
      ...detail.state.history,
    ]
      .reverse()
      .map(
        (h) =>
          `<article><strong>${esc(state.statuses[h.status])} · ${esc(h.reviewer || "未記入")}</strong><small>${esc(new Date(h.at).toLocaleString("ja-JP"))} · ${esc(h.head.slice(0, 7))}${h.dirty ? " + 未コミット変更" : ""}</small><p>${esc(h.notes)}</p><pre>${esc(h.evidence)}</pre></article>`,
      )
      .join("")}</details></section>`;
  $("#save-state").textContent = dirty
    ? "未保存の変更があります"
    : $("#save-state").textContent;
  $("#stale-warning").hidden =
    !detail.stale && draft.fingerprint === detail.fingerprint;
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
  const latest = detail.state.runs.at(-1),
    active = state.active?.itemId === selected;
  $("#send").disabled = sending || Boolean(state.active);
  $("#send").textContent = sending
    ? "送信中…"
    : state.active
      ? "作業中（中断して追加依頼可）"
      : "送信";
  const pane = $("#conversation");
  const html =
    detail.state.runs
      .map((run) => {
        const messages = run.messages?.length
          ? run.messages
          : run.report
            ? [{ text: run.report }]
            : [];
        return `<article class="conversation-turn" data-turn="${esc(run.id)}"><div class="chat-message human-message"><div class="message-meta"><strong>あなた</strong><time>${esc(new Date(run.startedAt).toLocaleString("ja-JP"))}</time><span>${run.mode === "read-only" ? "読み取りのみ" : "修正・実行可"}${run.continued === false ? " · 新しい会話" : ""}</span></div><pre>${esc(run.instruction || "この項目の確認を準備してください。")}</pre>${run.context ? `<details><summary>添付した編集中のメモ</summary><pre>${esc(run.context)}</pre></details>` : ""}</div>${messages.map((message) => `<div class="chat-message ${message.role === "human" ? "human-message" : "agent-message"} ${message.phase === "commentary" ? "commentary-message" : ""}"><div class="message-meta"><strong>${message.role === "human" ? "あなた（質問への回答）" : "Codex"}</strong>${message.phase === "commentary" ? "<span>作業経過</span>" : ""}</div><pre>${esc(message.text)}</pre></div>`).join("")}${run.preview ? `<div class="chat-message agent-message live-report"><strong>Codex · 応答中</strong><pre>${esc(run.preview)}</pre></div>` : ""}${run.error ? `<p class="warning">${esc(run.error)}</p>` : ""}<div class="turn-footer">${esc(runLabels[run.status] || run.status)}${run.changedWorkspace ? " · 作業ツリーの変更あり" : ""}</div>${run.diff ? `<details class="turn-diff" data-detail="${esc(run.id)}:diff"><summary>この作業の変更差分</summary><pre class="activity">${esc(run.diff)}</pre></details>` : ""}<details class="activity-details" data-detail="${esc(run.id)}:events"><summary>実行・検証の記録 (${run.events.length})</summary><div class="activity">${
          run.events
            .filter((e) => e.kind !== "message")
            .map(
              (e) =>
                `<article><small>${esc(new Date(e.at).toLocaleTimeString("ja-JP"))} · ${esc(e.kind)}</small><pre>${esc(e.detail)}</pre></article>`,
            )
            .join("") || "記録なし"
        }</div></details></article>`;
      })
      .join("") ||
    '<div class="run-empty">確認の準備、気になる点の相談、修正の依頼から始められます。</div>';
  if (pane.innerHTML !== html) {
    const bottom = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 60;
    const opened = new Set(
      [...pane.querySelectorAll("details[open][data-detail]")].map(
        (el) => el.dataset.detail,
      ),
    );
    pane.innerHTML = html;
    for (const el of pane.querySelectorAll("details[data-detail]"))
      el.open = opened.has(el.dataset.detail);
    if (bottom) pane.scrollTop = pane.scrollHeight;
  }
  preserveFocus(() => {
    $("#run").innerHTML =
      `${detail.workStale ? '<p class="warning">最後の応答後にコードが変わっています。現在のコードで再確認を依頼できます。</p>' : ""}${active ? `<div class="run-header"><strong>${esc(runLabels[latest.status])}</strong><button class="quiet danger" id="interrupt" data-run-id="${latest.id}">中断</button></div>` : ""}${(latest?.requests || []).map(renderRequest).join("")}`;
  });
}
async function select(id) {
  const request = ++selectionRequest;
  $("#detail").inert = true;
  try {
    const incoming = await api(`/api/items/${id}`);
    if (request !== selectionRequest) return;
    selected = id;
    sessionStorage.setItem("review:selected", id);
    detail = incoming;
    initDraft();
    if (humanDrafts.has(id)) {
      draft = structuredClone(humanDrafts.get(id));
      dirty = true;
    }
    renderList();
    renderDetail();
    error("");
    notice("");
  } finally {
    if (request === selectionRequest) $("#detail").inert = false;
  }
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
  const submitted = structuredClone(draft);
  const updated = await api(`/api/items/${id}`, "PATCH", submitted);
  localStorage.setItem("review:reviewer", submitted.reviewer);
  if (JSON.stringify(humanDrafts.get(id)) === JSON.stringify(submitted))
    humanDrafts.delete(id);
  if (selected === id) {
    detail = updated;
    if (JSON.stringify(draft) === JSON.stringify(submitted)) initDraft();
    else {
      draft.version = updated.state.version;
      humanDrafts.set(id, structuredClone(draft));
    }
    renderDetail();
  }
  notice("判断を保存しました。");
  await refresh();
}
function changed() {
  dirty = true;
  humanDrafts.set(selected, structuredClone(draft));
  $("#save-state").textContent = "未保存の変更があります";
}
document.addEventListener("input", (event) => {
  const el = event.target;
  if (el.id === "search") renderList();
  if (el.id === "instruction") composer().instruction = el.value;
  if (el.id === "mode") composer().mode = el.value;
  if (el.id === "new-conversation") composer().newConversation = el.checked;
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
    } else if (button.id === "save") {
      button.disabled = true;
      try {
        await save();
      } finally {
        button.disabled = false;
      }
    } else if (button.id === "recheck") {
      const incoming = await api(`/api/items/${selected}`);
      detail = incoming;
      draft.fingerprint = incoming.fingerprint;
      draft.version = incoming.state.version;
      draft.status = "reviewing";
      draft.checks = incoming.criteria.map(() => false);
      changed();
      renderDetail();
    } else if (button.dataset.message) {
      composer().instruction = button.dataset.message;
      $("#instruction").value = composer().instruction;
      $("#instruction").focus();
    } else if (button.id === "workspace-diff") {
      const result = await api("/api/diff");
      $("#source-title").textContent =
        `現在の作業ツリー全体の差分 · ${result.repository.head.slice(0, 7)} 以降`;
      $("#source-content").textContent =
        result.diff || "未コミットの変更はありません。";
      $("#source-dialog").showModal();
    } else if (button.id === "copy-prompt") {
      const result = await api(`/api/items/${selected}/prompt`, "POST", {
        ...composer(),
        context: draftContext(),
      });
      await navigator.clipboard.writeText(result.prompt);
      notice("依頼文をコピーしました。別のハーネスにも貼り付けられます。");
    } else if (button.id === "send") {
      if (sending || state.active) return;
      const itemId = selected,
        input = { ...composer() },
        context = draftContext();
      if (!input.instruction.trim())
        throw new Error("相談や作業の依頼を入力してください。");
      sending = true;
      renderRun();
      try {
        // Chat is independent of saving or completing the human decision form.
        const current = await api(`/api/items/${itemId}`);
        const result = await api(`/api/items/${itemId}/messages`, "POST", {
          instruction: input.instruction,
          mode: input.mode,
          continue: !input.newConversation,
          context,
          fingerprint: current.fingerprint,
        });
        if (
          composer(itemId).instruction === input.instruction &&
          !result.error
        ) {
          composer(itemId).instruction = "";
          composer(itemId).newConversation = false;
          if (selected === itemId) {
            $("#instruction").value = "";
            $("#new-conversation").checked = false;
          }
        }
        if (result.error) error(result.error);
        await refresh();
      } finally {
        sending = false;
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
  if (
    dirty ||
    humanDrafts.size ||
    [...composers.values()].some((input) => input.instruction.trim())
  ) {
    event.preventDefault();
    event.returnValue = "";
  }
});
document.addEventListener("keydown", (event) => {
  if (
    event.target.id === "instruction" &&
    (event.ctrlKey || event.metaKey) &&
    event.key === "Enter"
  ) {
    event.preventDefault();
    $("#send").click();
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
