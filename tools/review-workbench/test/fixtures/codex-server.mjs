// Exercises the real stdio bridge. Never invokes a model or executes a command.
import { createInterface } from "node:readline";
const mode = process.argv[2] || "approval";
const send = (message) => process.stdout.write(JSON.stringify(message) + "\n");
const notify = (method, params) =>
  send({
    method,
    params: { threadId: "thread-fixture", turnId: "turn-fixture", ...params },
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
    turn: { id: "turn-fixture", status: "completed", error: null },
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
    result({ thread: { id: "thread-fixture" } });
  } else if (message.method === "turn/start") {
    if (message.params.sandboxPolicy.networkAccess !== false)
      throw new Error("Network must default to disabled");
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
