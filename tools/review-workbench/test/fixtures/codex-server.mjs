// Exercises the real stdio bridge. Never invokes a model or executes a command.
// The conversation scenario edits only a designated file in a test checkout.
import { createInterface } from "node:readline";
import { readFileSync, writeFileSync } from "node:fs";
const mode = process.argv[2] || "approval";
let threadId = "thread-fixture",
  turnId = "turn-fixture",
  threadCount = 0,
  turnCount = 0;
const send = (message) => process.stdout.write(JSON.stringify(message) + "\n");
const notify = (method, params) =>
  send({
    method,
    params: { threadId, turnId, ...params },
  });
function finish(report) {
  notify("item/completed", {
    item: {
      type: "agentMessage",
      id: "message-1",
      phase: "final_answer",
      text: report,
    },
  });
  notify("turn/completed", {
    turn: { id: turnId, status: "completed", error: null },
  });
}
createInterface({ input: process.stdin }).on("line", (line) => {
  const message = JSON.parse(line);
  const result = (value) => send({ id: message.id, result: value });
  if (message.method === "initialize") result({ userAgent: "test-fixture" });
  else if (message.method === "account/read")
    result({ account: { type: "chatgpt", email: "review@example.invalid" } });
  else if (["thread/start", "thread/resume"].includes(message.method)) {
    if (
      message.params.approvalsReviewer !== "user" ||
      message.params.approvalPolicy !== "on-request"
    )
      throw new Error("Approvals must remain human controlled");
    if (mode === "conversation") {
      if (message.method === "thread/start")
        threadId = `thread-${++threadCount}`;
      else if (message.params.threadId !== threadId)
        throw new Error("Wrong conversation resumed");
      if (!message.params.developerInstructions.includes("ソース修正"))
        throw new Error("Source editing must be supported");
    }
    result({ thread: { id: threadId } });
  } else if (message.method === "turn/start") {
    if (message.params.sandboxPolicy.networkAccess !== false)
      throw new Error("Network must default to disabled");
    if (mode === "conversation") {
      turnId = `turn-${++turnCount}`;
      result({ turn: { id: turnId } });
      notify("turn/started", { turn: { id: turnId } });
      notify("item/completed", {
        item: {
          id: `comment-${turnCount}`,
          type: "agentMessage",
          phase: "commentary",
          text: "対象の実装を確認します。",
        },
      });
      const before = readFileSync("review-fixture.txt", "utf8");
      if (!/^label=(old|fixed)\n$/.test(before))
        throw new Error("Not an isolated review fixture");
      if (message.params.sandboxPolicy.type === "workspaceWrite") {
        if (
          !message.params.input[0].text.includes("ソース・テスト・文書を修正")
        )
          throw new Error("Prompt does not authorize source edits");
        writeFileSync("review-fixture.txt", "label=fixed\n");
        notify("turn/diff/updated", {
          diff: "diff --git a/review-fixture.txt b/review-fixture.txt\n-label=old\n+label=fixed\n",
        });
        notify("item/completed", {
          item: {
            id: "validation",
            type: "commandExecution",
            command: "fixture validation",
            exitCode: 0,
            aggregatedOutput: "label=fixed: PASS",
          },
        });
        finish(
          "label=fixed に修正しました。検証結果: PASS。画面で再確認してください。",
        );
      } else
        finish(
          `確認結果: ${before.trim()}。${threadId} の会話を継続しています。`,
        );
      return;
    }
    if (mode === "early") {
      finish("応答より先に完了した準備結果");
      result({ turn: { id: "turn-fixture" } });
      return;
    }
    result({ turn: { id: "turn-fixture" } });
    notify("turn/started", { turn: { id: "turn-fixture" } });
    if (mode === "crash") {
      process.exit(1);
    } else if (mode === "question")
      send({
        id: "question-1",
        method: "item/tool/requestUserInput",
        params: {
          threadId: "thread-fixture",
          turnId: "turn-fixture",
          itemId: "question",
          isBlocking: true,
          questions: [
            {
              id: "environment",
              header: "環境",
              question: "どの環境を確認しますか？",
              isSecret: false,
              options: [{ label: "ローカル", description: "手元のテスト環境" }],
            },
          ],
        },
      });
    else if (mode === "permissions")
      send({
        id: "permissions-1",
        method: "item/permissions/requestApproval",
        params: {
          threadId: "thread-fixture",
          turnId: "turn-fixture",
          itemId: "permissions",
          reason: "試験用の追加権限",
          permissions: { network: { enabled: true } },
        },
      });
    else {
      notify("item/started", {
        item: {
          id: "command-1",
          type: "commandExecution",
          command: "printf fixture",
          status: "inProgress",
        },
      });
      send({
        id: "approval-1",
        method: "item/commandExecution/requestApproval",
        params: {
          threadId: "thread-fixture",
          turnId: "turn-fixture",
          itemId: "command-1",
          command: "printf fixture",
          availableDecisions: ["accept", "decline"],
          reason: "確認用の操作",
        },
      });
    }
  } else if (message.method === "turn/interrupt") {
    result({});
    notify("turn/completed", {
      turn: { id: "turn-fixture", status: "interrupted" },
    });
  } else if (message.id === "approval-1")
    finish(
      `実行判断: ${message.result.decision}\n人間が確認する手順を準備しました。<script>unsafe()</script>`,
    );
  else if (message.id === "question-1")
    finish(`確認環境: ${message.result.answers.environment.answers[0]}`);
  else if (message.id === "permissions-1")
    finish(`許可内容: ${JSON.stringify(message.result)}`);
});
