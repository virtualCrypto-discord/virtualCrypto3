import { EventEmitter } from "node:events";

// A manually completed turn makes queue ordering deterministic in tests.
export class ControlledClient extends EventEmitter {
  constructor() {
    super();
    this.info = { connected: true, authenticated: true };
    this.calls = [];
    this.threads = 0;
    this.turns = 0;
    this.turn = null;
  }
  async connect() {
    return this.info;
  }
  async request(method, params) {
    this.calls.push({ method, params: structuredClone(params) });
    if (method === "thread/start")
      return { thread: { id: `thread-${++this.threads}` } };
    if (method === "thread/resume") return { thread: { id: params.threadId } };
    if (method === "turn/start") {
      if (this.turn) throw Error("Concurrent turns are not allowed");
      this.turn = { threadId: params.threadId, turnId: `turn-${++this.turns}` };
      this.emit("notification", {
        method: "turn/started",
        params: {
          threadId: this.turn.threadId,
          turn: { id: this.turn.turnId },
        },
      });
      return { turn: { id: this.turn.turnId } };
    }
    if (method === "turn/interrupt") {
      this.complete("interrupted");
      return {};
    }
    throw Error(`Unexpected request: ${method}`);
  }
  complete(status = "completed") {
    const turn = this.turn;
    if (!turn) throw Error("No turn to complete");
    this.turn = null;
    this.emit("notification", {
      method: "item/completed",
      params: {
        ...turn,
        item: {
          id: `message-${turn.turnId}`,
          type: "agentMessage",
          text: `Finished ${turn.turnId}`,
        },
      },
    });
    this.emit("notification", {
      method: "turn/completed",
      params: { threadId: turn.threadId, turn: { id: turn.turnId, status } },
    });
    return turn;
  }
  close() {
    this.turn = null;
    this.emit("failure", new Error("Fixture disconnected"));
  }
}
