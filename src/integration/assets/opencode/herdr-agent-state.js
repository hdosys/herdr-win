// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// add custom hooks/plugins beside this file instead of editing it.
// HERDR_INTEGRATION_ID=opencode
// HERDR_INTEGRATION_VERSION=10

import net from "node:net";

const SOURCE = "herdr:opencode";
const AGENT = "opencode";
let reportSeq = Date.now() * 1000;

const ERROR_FALLBACK_MS = 5_000;
const ERROR_RETRY_GRACE_MS = 1_000;
const ERROR_MAX_FALLBACK_MS = 2_147_483_647;

const CHILD_EVENT_STATES = new Map([
  ["permission.asked", "blocked"],
  ["question.asked", "blocked"],
  ["permission.replied", "working"],
  ["question.replied", "working"],
  ["question.rejected", "working"],
]);

const SESSION_STATE_BY_STATUS = new Map([
  ["idle", "idle"],
  ["active", "working"],
  ["busy", "working"],
  ["pending", "working"],
  ["retry", "working"],
  ["running", "working"],
  ["streaming", "working"],
  ["working", "working"],
]);

function nextReportSeq() {
  reportSeq += 1;
  return reportSeq;
}

function sessionIDFromProperties(properties) {
  return typeof properties?.sessionID === "string" && properties.sessionID
    ? properties.sessionID
    : undefined;
}

function sessionStatusKind(status) {
  return typeof status === "string" ? status.toLowerCase() : status?.type?.toLowerCase();
}

function stateFromSessionStatus(status) {
  const kind = sessionStatusKind(status);
  return typeof kind === "string"
    ? SESSION_STATE_BY_STATUS.get(kind)
    : undefined;
}

function promptEventKey(type, properties) {
  const kind = type?.split(".", 1)[0];
  if (kind !== "permission" && kind !== "question") {
    return undefined;
  }
  const requestID = type.endsWith(".asked")
    ? properties?.id
    : properties?.requestID;
  return typeof requestID === "string" && requestID
    ? `${kind}:${requestID}`
    : undefined;
}

export const HerdrAgentStatePlugin = async () => {
  if (
    process.env.HERDR_ENV !== "1" ||
    !process.env.HERDR_SOCKET_PATH ||
    !process.env.HERDR_PANE_ID
  ) {
    return {};
  }

  let requestChain = Promise.resolve();
  let currentRootSessionID;
  let unscopedErrorBlocked = false;
  let disposed = false;
  const sessionLifecycle = new Map();
  const activePrompts = new Map();
  const childParents = new Map();
  const retiredChildren = new Set();
  const serverCreatedSessions = new Set();
  const activeClients = new Map();

  function requestOnce(method, params) {
    if (disposed) {
      return Promise.resolve();
    }
    const paneId = process.env.HERDR_PANE_ID;
    const socketPath = process.env.HERDR_SOCKET_PATH;
    if (!paneId || !socketPath) {
      return Promise.resolve();
    }

    const socketEndpoint =
      process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
    const request = {
      id: `${SOURCE}:${Date.now()}:${Math.floor(Math.random() * 1_000_000)
        .toString()
        .padStart(6, "0")}`,
      method,
      params: {
        pane_id: paneId,
        source: SOURCE,
        agent: AGENT,
        seq: nextReportSeq(),
        ...params,
      },
    };

    return new Promise((resolve) => {
      if (disposed) {
        resolve();
        return;
      }

      let client;
      let finished = false;
      const finish = () => {
        if (finished) {
          return;
        }
        finished = true;
        if (client) {
          activeClients.delete(client);
          client.destroy();
        }
        resolve();
      };

      client = net.createConnection(socketEndpoint, () => {
        if (disposed) {
          finish();
          return;
        }
        client.write(`${JSON.stringify(request)}\n`);
      });
      activeClients.set(client, finish);
      if (disposed) {
        finish();
        return;
      }
      client.setTimeout(500, finish);
      client.on("data", finish);
      client.on("error", finish);
      client.on("end", finish);
      client.on("close", finish);
    });
  }

  function request(method, params) {
    const pending = requestChain.then(() => {
      if (disposed) {
        return;
      }
      return requestOnce(method, params);
    });
    requestChain = pending.catch(() => {});
    return pending;
  }

  function reportSession(sessionID, sessionStartSource) {
    if (disposed || !sessionID) {
      return Promise.resolve();
    }
    const params = { agent_session_id: sessionID };
    if (sessionStartSource) {
      params.session_start_source = sessionStartSource;
    }
    return request("pane.report_agent_session", params);
  }

  function reportState(state, sessionID) {
    if (disposed) {
      return Promise.resolve();
    }
    const params = { state };
    if (sessionID) {
      params.agent_session_id = sessionID;
    }
    return request("pane.report_agent", params);
  }

  function lifecycleFor(sessionID) {
    let lifecycle = sessionLifecycle.get(sessionID);
    if (!lifecycle) {
      lifecycle = {
        pendingError: false,
        timer: undefined,
        retryNext: undefined,
        blocked: false,
      };
      sessionLifecycle.set(sessionID, lifecycle);
    }
    return lifecycle;
  }

  function clearPendingError(lifecycle) {
    if (lifecycle.timer !== undefined) {
      clearTimeout(lifecycle.timer);
    }
    lifecycle.timer = undefined;
    lifecycle.pendingError = false;
  }

  function clearSessionLifecycle(sessionID) {
    if (!sessionID) {
      return;
    }
    const lifecycle = sessionLifecycle.get(sessionID);
    if (lifecycle) {
      clearPendingError(lifecycle);
      sessionLifecycle.delete(sessionID);
    }
  }

  function clearPromptsForSession(sessionID) {
    if (!sessionID) {
      return;
    }
    for (const [key, ownerSessionID] of activePrompts) {
      if (ownerSessionID === sessionID) {
        activePrompts.delete(key);
      }
    }
  }

  function updatePromptState(type, properties, sessionID) {
    const key = promptEventKey(type, properties);
    if (!key) {
      return undefined;
    }
    if (type.endsWith(".asked")) {
      activePrompts.set(key, sessionID);
      return "blocked";
    }
    activePrompts.delete(key);
    return activePrompts.size > 0 ? "blocked" : "working";
  }

  function retireChild(sessionID) {
    if (!sessionID) {
      return;
    }
    clearPromptsForSession(sessionID);
    childParents.delete(sessionID);
    retiredChildren.add(sessionID);
  }

  function retireChildrenOutsideRoot(rootSessionID) {
    for (const [childSessionID, parentSessionID] of childParents) {
      if (parentSessionID !== rootSessionID) {
        retireChild(childSessionID);
      }
    }
  }

  function retireAllChildren() {
    for (const childSessionID of [...childParents.keys()]) {
      retireChild(childSessionID);
    }
  }

  function establishRootSession(sessionID) {
    if (!sessionID) {
      return;
    }
    serverCreatedSessions.delete(sessionID);
    if (currentRootSessionID && currentRootSessionID !== sessionID) {
      clearSessionLifecycle(currentRootSessionID);
      clearPromptsForSession(currentRootSessionID);
    }
    retireChildrenOutsideRoot(sessionID);
    clearSessionLifecycle(sessionID);
    currentRootSessionID = sessionID;
  }

  function acceptsRootEvent(sessionID) {
    if (!sessionID) {
      return true;
    }
    if (!currentRootSessionID) {
      establishRootSession(sessionID);
      return true;
    }
    return sessionID === currentRootSessionID;
  }

  function markSessionContinuing(sessionID, status) {
    if (!sessionID) {
      return;
    }
    if (sessionStatusKind(status) !== "retry") {
      clearSessionLifecycle(sessionID);
      return;
    }

    const lifecycle = lifecycleFor(sessionID);
    clearPendingError(lifecycle);
    lifecycle.blocked = false;
    lifecycle.retryNext =
      typeof status === "object" && typeof status?.next === "number"
        ? status.next
        : undefined;
  }

  function errorFallbackDelay(lifecycle) {
    const retryRemaining =
      typeof lifecycle.retryNext === "number"
        ? Math.max(0, lifecycle.retryNext - Date.now())
        : 0;
    return Math.min(
      ERROR_MAX_FALLBACK_MS,
      Math.max(ERROR_FALLBACK_MS, retryRemaining + ERROR_RETRY_GRACE_MS),
    );
  }

  async function confirmPendingError(sessionID) {
    if (disposed) {
      return false;
    }
    const lifecycle = sessionLifecycle.get(sessionID);
    if (!lifecycle?.pendingError) {
      return false;
    }
    clearPendingError(lifecycle);
    lifecycle.retryNext = undefined;
    if (!lifecycle.blocked) {
      lifecycle.blocked = true;
      await reportState("blocked", sessionID);
    }
    return true;
  }

  function deferSessionError(sessionID) {
    if (disposed) {
      return;
    }
    const lifecycle = lifecycleFor(sessionID);
    if (lifecycle.blocked) {
      return;
    }
    clearPendingError(lifecycle);
    lifecycle.pendingError = true;
    lifecycle.timer = setTimeout(() => {
      if (!disposed) {
        void confirmPendingError(sessionID);
      }
    }, errorFallbackDelay(lifecycle));
    lifecycle.timer.unref?.();
  }

  async function reportContinuing(state, sessionID, status) {
    markSessionContinuing(sessionID, status);
    if (!unscopedErrorBlocked && activePrompts.size === 0) {
      await reportState(state, sessionID);
    }
  }

  async function reportIdleOrConfirmError(sessionID) {
    if (unscopedErrorBlocked || activePrompts.size > 0) {
      return;
    }
    const lifecycle = sessionID ? sessionLifecycle.get(sessionID) : undefined;
    if (sessionID && lifecycle?.pendingError) {
      await confirmPendingError(sessionID);
      return;
    }
    if (lifecycle?.blocked) {
      return;
    }
    clearSessionLifecycle(sessionID);
    await reportState("idle", sessionID);
  }

  function dispose() {
    if (disposed) {
      return;
    }
    disposed = true;
    for (const lifecycle of sessionLifecycle.values()) {
      clearPendingError(lifecycle);
    }
    sessionLifecycle.clear();
    activePrompts.clear();
    childParents.clear();
    retiredChildren.clear();
    serverCreatedSessions.clear();
    currentRootSessionID = undefined;
    unscopedErrorBlocked = false;
    for (const finish of [...activeClients.values()]) {
      finish();
    }
    activeClients.clear();
    requestChain = Promise.resolve();
  }

  return {
    "chat.message": async ({ sessionID }) => {
      if (
        disposed ||
        (sessionID &&
          (retiredChildren.has(sessionID) || childParents.has(sessionID)))
      ) {
        return;
      }
      establishRootSession(sessionID);
      if (!unscopedErrorBlocked && activePrompts.size === 0) {
        await reportState("working", sessionID);
      }
    },
    event: async ({ event }) => {
      if (disposed) {
        return;
      }
      const type = event?.type;
      const properties = event?.properties ?? {};
      const sessionID = sessionIDFromProperties(properties);
      const info = properties.info;

      if (type === "session.deleted" && info?.id && info.parentID) {
        retireChild(info.id);
        return;
      }

      if (
        (sessionID && retiredChildren.has(sessionID)) ||
        (info?.id && retiredChildren.has(info.id))
      ) {
        return;
      }

      if (info?.id && info.parentID) {
        if (!currentRootSessionID || info.parentID === currentRootSessionID) {
          childParents.set(info.id, info.parentID);
        } else {
          retireChild(info.id);
        }
        return;
      }

      if (sessionID && childParents.has(sessionID)) {
        if (type === "session.deleted") {
          retireChild(sessionID);
          return;
        }
        const state = updatePromptState(type, properties, sessionID)
          ?? CHILD_EVENT_STATES.get(type);
        if (state && !unscopedErrorBlocked) {
          await reportState(state, currentRootSessionID);
        }
        return;
      }

      if (
        type !== "session.created" &&
        type !== "session.updated" &&
        type !== "session.deleted" &&
        !acceptsRootEvent(sessionID)
      ) {
        return;
      }

      updatePromptState(type, properties, sessionID);

      switch (type) {
        case "session.created":
          // Creation is server-global, so another attached client may own it.
          // The TUI plugin reports the root selected in this pane.
          if (sessionID) {
            serverCreatedSessions.add(sessionID);
          }
          break;
        case "session.updated":
          if (
            sessionID &&
            !currentRootSessionID &&
            !serverCreatedSessions.has(sessionID)
          ) {
            establishRootSession(sessionID);
            await reportSession(sessionID);
          }
          break;
        case "session.status": {
          const state = stateFromSessionStatus(properties.status);
          if (state === "working") {
            await reportContinuing(state, sessionID, properties.status);
          } else if (state === "idle") {
            await reportIdleOrConfirmError(sessionID);
          } else {
            await reportSession(sessionID);
          }
          break;
        }
        case "tool.execute.before":
        case "tool.execute.after":
        case "permission.replied":
        case "question.replied":
        case "question.rejected":
        case "session.compacted":
          await reportContinuing("working", sessionID, "working");
          break;
        case "permission.asked":
        case "question.asked":
          clearSessionLifecycle(sessionID);
          await reportState("blocked", sessionID);
          break;
        case "session.error":
          if (!sessionID) {
            unscopedErrorBlocked = true;
            await reportState("blocked");
            break;
          }
          deferSessionError(sessionID);
          break;
        case "session.idle":
          await reportIdleOrConfirmError(sessionID);
          break;
        case "session.deleted":
          serverCreatedSessions.delete(sessionID);
          clearSessionLifecycle(sessionID);
          clearPromptsForSession(sessionID);
          if (sessionID && sessionID === currentRootSessionID) {
            currentRootSessionID = undefined;
            retireAllChildren();
          }
          break;
        default:
          break;
      }
    },
    dispose,
  };
};
