import { beforeEach, expect, mock, test, vi } from "bun:test";

const requests: unknown[] = [];
const clients: FakeClient[] = [];
const requestWaiters: Array<() => void> = [];
let autoAcknowledge = true;
let importCounter = 0;

type FakeClient = {
  destroyed: boolean;
  emit: (event: string) => void;
};

mock.module("node:net", () => ({
  default: {
    createConnection(_path: string, onConnect: () => void) {
      const handlers = new Map<string, () => void>();
      const client = {
        destroyed: false,
        write(input: string) {
          requests.push(JSON.parse(input.trim()));
          requestWaiters.shift()?.();
          if (autoAcknowledge) {
            queueMicrotask(() => client.emit("data"));
          }
        },
        setTimeout() {},
        on(event: string, handler: () => void) {
          handlers.set(event, handler);
        },
        destroy() {
          client.destroyed = true;
        },
        emit(event: string) {
          handlers.get(event)?.();
        },
      };
      clients.push(client);
      queueMicrotask(onConnect);
      return client;
    },
  },
}));

beforeEach(() => {
  requests.length = 0;
  clients.length = 0;
  requestWaiters.length = 0;
  autoAcknowledge = true;
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = "test.sock";
  process.env.HERDR_PANE_ID = "test:p1";
});

async function loadPluginFactory() {
  importCounter += 1;
  const { HerdrAgentStatePlugin } = await import(`./herdr-agent-state.js?test=${importCounter}`);
  return HerdrAgentStatePlugin;
}

async function loadPlugin() {
  return (await loadPluginFactory())();
}

function waitForNextRequest(): Promise<void> {
  return new Promise((resolve) => requestWaiters.push(resolve));
}

function sessionStatusEvent(sessionID: string, status: Record<string, unknown>) {
  return {
    event: {
      type: "session.status",
      properties: { sessionID, status },
    },
  };
}

function apiErrorEvent(sessionID?: string) {
  return {
    event: {
      type: "session.error",
      properties: {
        ...(sessionID ? { sessionID } : {}),
        error: {
          name: "APIError",
          data: {
            message: "Service unavailable",
            statusCode: 503,
            isRetryable: true,
          },
        },
      },
    },
  };
}

test("serializes lifecycle reports", async () => {
  autoAcknowledge = false;
  const plugin = await loadPlugin();
  const firstDispatched = waitForNextRequest();
  const working = plugin.event({
    event: {
      type: "session.status",
      properties: { sessionID: "root-session", status: { type: "busy" } },
    },
  });
  await firstDispatched;

  const secondDispatched = waitForNextRequest();
  const idle = plugin.event({
    event: {
      type: "session.status",
      properties: { sessionID: "root-session", status: { type: "idle" } },
    },
  });
  expect(clients).toHaveLength(1);

  clients[0]?.emit("data");
  await secondDispatched;
  expect(clients).toHaveLength(2);
  clients[1]?.emit("data");
  await Promise.all([working, idle]);

  expect(requests.map(requestState)).toEqual(["working", "idle"]);
  const sequences = requests.map(requestSeq);
  expect(sequences[0]).toEqual(expect.any(Number));
  expect(sequences[1]).toBe((sequences[0] as number) + 1);
});

test("ignores session.updated once the current root is established", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.status",
      properties: { sessionID: "root-session", status: { type: "busy" } },
    },
  });
  await plugin.event({
    event: { type: "session.updated", properties: { sessionID: "root-session" } },
  });
  await plugin.event({
    event: { type: "session.updated", properties: { sessionID: "replacement-session" } },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session"]);
});

test("does not classify server activity in another root session as a selection", async () => {
  const plugin = await loadPlugin();

  await plugin["chat.message"]({ sessionID: "visible-session" });
  await plugin["chat.message"]({ sessionID: "attached-client-session" });

  expect(requests.map(requestMethod)).toEqual([
    "pane.report_agent",
    "pane.report_agent",
  ]);
  expect(requests.map(requestSessionID)).toEqual([
    "visible-session",
    "attached-client-session",
  ]);
});

test("does not classify server-global root creation as a local selection", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: { type: "session.created", properties: { sessionID: "attached-session" } },
  });
  await plugin.event({
    event: { type: "session.updated", properties: { sessionID: "attached-session" } },
  });
  await plugin["chat.message"]({ sessionID: "attached-session" });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestSessionID)).toEqual(["attached-session"]);
});

test("reports retry status as working", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.status",
      properties: { sessionID: "root-session", status: { type: "retry" } },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestState)).toEqual(["working"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session"]);
});

test("error followed by retry reports working without blocked", async () => {
  vi.useFakeTimers();
  try {
    const plugin = await loadPlugin();

    await plugin.event(apiErrorEvent("root-session"));
    expect(requests).toHaveLength(0);

    await plugin.event(
      sessionStatusEvent("root-session", {
        type: "retry",
        attempt: 1,
        message: "Service unavailable",
        next: Date.now() + 2_000,
      }),
    );
    vi.runAllTimers();
    await Promise.resolve();

    expect(requests.map(requestState)).toEqual(["working"]);
    expect(requests.map(requestSessionID)).toEqual(["root-session"]);
  } finally {
    vi.useRealTimers();
  }
});

test("retry then error then busy never reports blocked", async () => {
  vi.useFakeTimers();
  try {
    const plugin = await loadPlugin();

    await plugin.event(
      sessionStatusEvent("root-session", {
        type: "retry",
        attempt: 1,
        message: "Service unavailable",
        next: Date.now() + 30_000,
      }),
    );
    await plugin.event(apiErrorEvent("root-session"));
    vi.advanceTimersByTime(5_000);
    await Promise.resolve();
    expect(requests.map(requestState)).toEqual(["working"]);

    await plugin.event(sessionStatusEvent("root-session", { type: "busy" }));
    vi.runAllTimers();
    await Promise.resolve();

    expect(requests.map(requestState)).toEqual(["working", "working"]);
    expect(requests.map(requestState)).not.toContain("blocked");
  } finally {
    vi.useRealTimers();
  }
});

test("error then idle reports blocked once without idle overwrite", async () => {
  const plugin = await loadPlugin();

  await plugin.event(apiErrorEvent("root-session"));
  await plugin.event(sessionStatusEvent("root-session", { type: "idle" }));
  await plugin.event({
    event: { type: "session.idle", properties: { sessionID: "root-session" } },
  });

  expect(requests.map(requestState)).toEqual(["blocked"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session"]);
});

test("error without follow-up reports blocked through bounded fallback", async () => {
  vi.useFakeTimers();
  try {
    const plugin = await loadPlugin();
    await plugin.event(apiErrorEvent("root-session"));
    expect(requests).toHaveLength(0);

    const dispatched = waitForNextRequest();
    vi.runAllTimers();
    await dispatched;

    expect(requests.map(requestState)).toEqual(["blocked"]);
    expect(requests.map(requestSessionID)).toEqual(["root-session"]);
  } finally {
    vi.useRealTimers();
  }
});

test("unrelated status cannot cancel another session pending error", async () => {
  const plugin = await loadPlugin();

  await plugin.event(apiErrorEvent("first-session"));
  await plugin.event(
    sessionStatusEvent("second-session", {
      type: "retry",
      attempt: 1,
      message: "Service unavailable",
      next: Date.now() + 2_000,
    }),
  );
  await plugin.event(sessionStatusEvent("first-session", { type: "idle" }));

  expect(requests.map(requestState)).toEqual(["blocked"]);
  expect(requests.map(requestSessionID)).toEqual(["first-session"]);
});

test("session deletion clears a pending error fallback", async () => {
  vi.useFakeTimers();
  try {
    const plugin = await loadPlugin();
    await plugin.event(apiErrorEvent("root-session"));
    await plugin.event({
      event: {
        type: "session.deleted",
        properties: { sessionID: "root-session" },
      },
    });

    vi.runAllTimers();
    await Promise.resolve();
    expect(requests).toHaveLength(0);
  } finally {
    vi.useRealTimers();
  }
});

test("new session retires old pending lifecycle and stale events", async () => {
  vi.useFakeTimers();
  try {
    const plugin = await loadPlugin();
    await plugin.event(apiErrorEvent("old-session"));
    await plugin["chat.message"]({ sessionID: "new-session" });

    await plugin.event(sessionStatusEvent("old-session", { type: "idle" }));
    await plugin.event(apiErrorEvent("old-session"));
    await plugin.event({
      event: {
        type: "session.idle",
        properties: { sessionID: "old-session" },
      },
    });
    vi.runAllTimers();
    await Promise.resolve();

    expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
    expect(requests.map(requestSessionID)).toEqual(["new-session"]);
    expect(requests.map(requestState)).toEqual(["working"]);
  } finally {
    vi.useRealTimers();
  }
});

test("unscoped error remains blocked across current root busy", async () => {
  const plugin = await loadPlugin();
  await plugin.event({
    event: {
      type: "session.created",
      properties: { sessionID: "root-session" },
    },
  });
  requests.length = 0;

  await plugin.event(apiErrorEvent());
  await plugin.event(sessionStatusEvent("root-session", { type: "busy" }));

  expect(requests.map(requestState)).toEqual(["blocked"]);
  expect(requests.map(requestSessionID)).toEqual([undefined]);
});

test("dispose cancels pending fallback and ignores later events", async () => {
  vi.useFakeTimers();
  try {
    const plugin = await loadPlugin();
    await plugin.event(apiErrorEvent("root-session"));
    await plugin.dispose();
    vi.runAllTimers();
    await Promise.resolve();
    await plugin.event(sessionStatusEvent("root-session", { type: "idle" }));

    expect(requests).toHaveLength(0);
  } finally {
    vi.useRealTimers();
  }
});

test("session.updated establishes only the initial current session", async () => {
  const plugin = await loadPlugin();
  await plugin.event({
    event: {
      type: "session.updated",
      properties: { sessionID: "startup-root" },
    },
  });
  await plugin.event({
    event: {
      type: "session.updated",
      properties: { sessionID: "background-session" },
    },
  });
  await plugin.event(sessionStatusEvent("startup-root", { type: "busy" }));
  await plugin.event(sessionStatusEvent("background-session", { type: "busy" }));

  expect(requests.map(requestMethod)).toEqual([
    "pane.report_agent_session",
    "pane.report_agent",
  ]);
  expect(requests.map(requestSessionID)).toEqual(["startup-root", "startup-root"]);
});

test("background session.updated cannot steal a pending terminal root", async () => {
  const plugin = await loadPlugin();
  await plugin.event(apiErrorEvent("root-a"));
  await plugin.event({
    event: {
      type: "session.updated",
      properties: { sessionID: "root-b" },
    },
  });
  await plugin.event(sessionStatusEvent("root-a", { type: "idle" }));

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestState)).toEqual(["blocked"]);
  expect(requests.map(requestSessionID)).toEqual(["root-a"]);
});

test("chat.message establishes new root ownership", async () => {
  const plugin = await loadPlugin();
  await plugin.event({
    event: {
      type: "session.created",
      properties: { sessionID: "root-a" },
    },
  });
  await plugin["chat.message"]({ sessionID: "root-b" });
  await plugin.event(sessionStatusEvent("root-a", { type: "busy" }));
  await plugin.event(sessionStatusEvent("root-b", { type: "busy" }));

  expect(requests.map(requestMethod)).toEqual([
    "pane.report_agent",
    "pane.report_agent",
  ]);
  expect(requests.map(requestSessionID)).toEqual(["root-b", "root-b"]);
});

test("same-root chat cannot clear an unscoped error block", async () => {
  const plugin = await loadPlugin();
  await plugin.event({
    event: {
      type: "session.created",
      properties: { sessionID: "root-session" },
    },
  });
  requests.length = 0;

  await plugin.event(apiErrorEvent());
  await plugin["chat.message"]({ sessionID: "root-session" });

  expect(requests.map(requestState)).toEqual(["blocked"]);
  expect(requests.map(requestSessionID)).toEqual([undefined]);
});

test("old child reply is dropped after root replacement", async () => {
  const plugin = await loadPlugin();
  await plugin["chat.message"]({ sessionID: "old-root" });
  requests.length = 0;
  await plugin.event({
    event: {
      type: "session.created",
      properties: {
        sessionID: "old-child",
        info: { id: "old-child", parentID: "old-root" },
      },
    },
  });
  await plugin["chat.message"]({ sessionID: "new-root" });
  await plugin.event({
    event: {
      type: "permission.replied",
      properties: { sessionID: "old-child" },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestSessionID)).toEqual(["new-root"]);
});

test("canonical child deletion removes ownership before trailing reply", async () => {
  const plugin = await loadPlugin();
  await plugin["chat.message"]({ sessionID: "root-session" });
  requests.length = 0;
  await plugin.event({
    event: {
      type: "session.created",
      properties: {
        sessionID: "child-session",
        info: { id: "child-session", parentID: "root-session" },
      },
    },
  });
  await plugin.event({
    event: {
      type: "session.deleted",
      properties: {
        sessionID: "child-session",
        info: { id: "child-session", parentID: "root-session" },
      },
    },
  });
  await plugin.event({
    event: {
      type: "question.replied",
      properties: { sessionID: "child-session" },
    },
  });

  expect(requests).toHaveLength(0);
});

test("no-root deleted child stays retired through trailing activity", async () => {
  const plugin = await loadPlugin();
  await plugin.event({
    event: {
      type: "session.created",
      properties: {
        sessionID: "child-session",
        info: { id: "child-session", parentID: "unknown-root" },
      },
    },
  });
  await plugin.event({
    event: {
      type: "session.deleted",
      properties: {
        sessionID: "child-session",
        info: { id: "child-session", parentID: "unknown-root" },
      },
    },
  });
  await plugin.event({
    event: {
      type: "question.replied",
      properties: { sessionID: "child-session" },
    },
  });
  await plugin["chat.message"]({ sessionID: "child-session" });

  expect(requests).toHaveLength(0);
});

test("initial root retires children learned for another parent", async () => {
  const plugin = await loadPlugin();
  await plugin.event({
    event: {
      type: "session.created",
      properties: {
        sessionID: "old-child",
        info: { id: "old-child", parentID: "old-parent" },
      },
    },
  });
  await plugin.event({
    event: {
      type: "session.updated",
      properties: { sessionID: "new-root" },
    },
  });
  await plugin.event({
    event: {
      type: "question.replied",
      properties: { sessionID: "old-child" },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent_session"]);
  expect(requests.map(requestSessionID)).toEqual(["new-root"]);
  expect(requests.map(requestState)).toEqual([undefined]);
});

test("initial status root retires children learned for another parent", async () => {
  const plugin = await loadPlugin();
  await plugin.event({
    event: {
      type: "session.created",
      properties: {
        sessionID: "old-child",
        info: { id: "old-child", parentID: "old-root" },
      },
    },
  });
  await plugin.event(sessionStatusEvent("new-root", { type: "busy" }));
  await plugin.event({
    event: {
      type: "question.replied",
      properties: { sessionID: "old-child" },
    },
  });
  await plugin["chat.message"]({ sessionID: "old-child" });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestState)).toEqual(["working"]);
  expect(requests.map(requestSessionID)).toEqual(["new-root"]);
});

test("dispose drops queued statuses before socket dispatch", async () => {
  autoAcknowledge = false;
  const plugin = await loadPlugin();
  const first = plugin.event(sessionStatusEvent("root-session", { type: "busy" }));
  const second = plugin.event(sessionStatusEvent("root-session", { type: "busy" }));

  await plugin.dispose();
  await Promise.all([first, second]);

  expect(requests).toHaveLength(0);
  expect(clients).toHaveLength(0);
});

test("dispose destroys an in-flight socket and resolves its request", async () => {
  autoAcknowledge = false;
  const plugin = await loadPlugin();
  const dispatched = waitForNextRequest();
  const pending = plugin.event(sessionStatusEvent("root-session", { type: "busy" }));
  await dispatched;

  expect(clients).toHaveLength(1);
  expect(clients[0]?.destroyed).toBe(false);
  await plugin.dispose();
  await pending;

  expect(clients[0]?.destroyed).toBe(true);
  expect(requests).toHaveLength(1);
});

test("a cached module can create a fresh plugin after disposal", async () => {
  const factory = await loadPluginFactory();
  const first = await factory();
  await first.dispose();

  const second = await factory();
  await second.event(sessionStatusEvent("root-session", { type: "busy" }));

  expect(requests.map(requestState)).toEqual(["working"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session"]);
});

test("reports child prompts without replacing the root session", async () => {
  const plugin = await loadPlugin();
  await plugin["chat.message"]({ sessionID: "root-session" });
  requests.length = 0;

  await plugin.event({
    event: {
      type: "session.created",
      properties: {
        sessionID: "child-session",
        info: { id: "child-session", parentID: "root-session" },
      },
    },
  });

  for (const type of ["permission.asked", "question.asked"]) {
    await plugin.event({ event: { type, properties: { sessionID: "child-session" } } });
  }
  for (const type of ["permission.replied", "question.replied", "question.rejected"]) {
    await plugin.event({ event: { type, properties: { sessionID: "child-session" } } });
  }

  expect(requests.map(requestState)).toEqual([
    "blocked",
    "blocked",
    "working",
    "working",
    "working",
  ]);
  expect(requests.map(requestSessionID)).toEqual([
    "root-session",
    "root-session",
    "root-session",
    "root-session",
    "root-session",
  ]);
});

test("root prompts stay blocked until every request completes", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "permission.asked",
      properties: { id: "permission-1", sessionID: "root-session" },
    },
  });
  await plugin.event({
    event: {
      type: "question.asked",
      properties: { id: "question-1", sessionID: "root-session" },
    },
  });
  await plugin.event({
    event: {
      type: "permission.replied",
      properties: { requestID: "permission-1", sessionID: "root-session" },
    },
  });
  await plugin.event(sessionStatusEvent("root-session", { type: "busy" }));
  await plugin.event(sessionStatusEvent("root-session", { type: "idle" }));
  await plugin.event({
    event: {
      type: "question.rejected",
      properties: { requestID: "question-1", sessionID: "root-session" },
    },
  });

  expect(requests.map(requestState)).toEqual(["blocked", "blocked", "working"]);
  expect(requests.map(requestSessionID)).toEqual([
    "root-session",
    "root-session",
    "root-session",
  ]);
});

test("child prompt blocks survive root statuses until the child replies", async () => {
  const plugin = await loadPlugin();
  await plugin.event({
    event: {
      type: "session.created",
      properties: { sessionID: "root-session" },
    },
  });
  await plugin["chat.message"]({ sessionID: "root-session" });
  requests.length = 0;

  await plugin.event({
    event: {
      type: "session.created",
      properties: {
        sessionID: "child-session",
        info: { id: "child-session", parentID: "root-session" },
      },
    },
  });
  await plugin.event({
    event: {
      type: "question.asked",
      properties: { id: "question-1", sessionID: "child-session" },
    },
  });
  await plugin.event(sessionStatusEvent("root-session", { type: "busy" }));
  await plugin.event(sessionStatusEvent("root-session", { type: "idle" }));
  await plugin.event({
    event: {
      type: "question.replied",
      properties: { requestID: "question-1", sessionID: "child-session" },
    },
  });

  expect(requests.map(requestState)).toEqual(["blocked", "working"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session", "root-session"]);
});

function requestMethod(request: unknown): unknown {
  return isRecord(request) ? request.method : undefined;
}

function requestState(request: unknown): unknown {
  return requestParam(request, "state");
}

function requestSeq(request: unknown): unknown {
  return requestParam(request, "seq");
}

function requestSessionID(request: unknown): unknown {
  return requestParam(request, "agent_session_id");
}

function requestParam(request: unknown, name: string): unknown {
  if (!isRecord(request) || !isRecord(request.params)) {
    return undefined;
  }
  return request.params[name];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
