// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// add custom hooks/plugins beside this file instead of editing it.
// HERDR_INTEGRATION_ID=opencode
// HERDR_INTEGRATION_VERSION=15

import { createHash } from "node:crypto";
import net from "node:net";

const SOURCE = "herdr:opencode";
const AGENT = "opencode";
let reportSeq = Date.now() * 1000;

const ERROR_FALLBACK_MS = 5_000;
const ERROR_RETRY_GRACE_MS = 1_000;
const ERROR_MAX_FALLBACK_MS = 2_147_483_647;
const SUBAGENT_SESSION_ENV = "HERDR_OPENCODE_SUBAGENT_SESSION_ID";
const SUBAGENT_START_TIMEOUT_MS = 30_000;
const MAX_RESPONSE_CHARACTERS = 64 * 1024;
const MIN_READABLE_SUBAGENT_COLUMNS = 80;
const SERVER_PROBE_TIMEOUT_MS = 500;

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

function isSessionAbort(error) {
  return error?.name === "MessageAbortedError";
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

export const HerdrAgentStatePlugin = async ({ client, directory, serverUrl } = {}) => {
  if (
    process.env.HERDR_ENV !== "1" ||
    !process.env.HERDR_SOCKET_PATH ||
    !process.env.HERDR_PANE_ID
  ) {
    return {};
  }

  let requestChain = Promise.resolve();
  let panePlacementChain = Promise.resolve();
  let currentRootSessionID;
  let unscopedErrorBlocked = false;
  let disposing = false;
  let disposed = false;
  const sessionLifecycle = new Map();
  const activePrompts = new Map();
  const children = new Map();
  const retiredChildren = new Set();
  const serverCreatedSessions = new Set();
  const activeClients = new Map();

  const attachServerUrl = serverUrl instanceof URL ? serverUrl.toString() : undefined;
  const defaultDirectory = typeof directory === "string" && directory ? directory : undefined;

  function requestOnce(method, params, allowWhileDisposing = false) {
    if (disposed || (disposing && !allowWhileDisposing)) {
      return Promise.resolve();
    }
    const socketPath = process.env.HERDR_SOCKET_PATH;
    if (!socketPath) {
      return Promise.resolve();
    }

    const socketEndpoint =
      process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
    const request = {
      id: `${SOURCE}:${Date.now()}:${Math.floor(Math.random() * 1_000_000)
        .toString()
        .padStart(6, "0")}`,
      method,
      params,
    };

    return new Promise((resolve) => {
      if (disposed || (disposing && !allowWhileDisposing)) {
        resolve();
        return;
      }

      let client;
      let responseBuffer = "";
      let finished = false;
      const finish = (response) => {
        if (finished) {
          return;
        }
        finished = true;
        if (client) {
          activeClients.delete(client);
          client.destroy();
        }
        resolve(response);
      };
      const finishFromBuffer = () => {
        const line = responseBuffer.trim();
        if (line) {
          try {
            const response = JSON.parse(line);
            if (response?.id === request.id) {
              finish(response);
              return;
            }
          } catch {
            // A closed or timed-out socket without a complete response is best effort.
          }
        }
        finish();
      };
      const receive = (chunk) => {
        if (chunk === undefined) {
          return;
        }
        responseBuffer += chunk.toString();
        if (responseBuffer.length > MAX_RESPONSE_CHARACTERS) {
          finish();
          return;
        }
        let newline = responseBuffer.indexOf("\n");
        while (newline >= 0) {
          const line = responseBuffer.slice(0, newline).trim();
          responseBuffer = responseBuffer.slice(newline + 1);
          if (line) {
            try {
              const response = JSON.parse(line);
              if (response?.id === request.id) {
                finish(response);
                return;
              }
            } catch {
              // Ignore unrelated malformed lines and keep the bounded response open.
            }
          }
          newline = responseBuffer.indexOf("\n");
        }
      };

      client = net.createConnection(socketEndpoint, () => {
        if (disposed || (disposing && !allowWhileDisposing)) {
          finish();
          return;
        }
        client.write(`${JSON.stringify(request)}\n`);
      });
      activeClients.set(client, finish);
      if (disposed || (disposing && !allowWhileDisposing)) {
        finish();
        return;
      }
      client.setTimeout(500, finish);
      client.on("data", receive);
      client.on("error", finish);
      client.on("end", finishFromBuffer);
      client.on("close", finishFromBuffer);
    });
  }

  function request(method, params, allowWhileDisposing = false) {
    const pending = requestChain.then(() => {
      if (disposed || (disposing && !allowWhileDisposing)) {
        return;
      }
      return requestOnce(method, params, allowWhileDisposing);
    });
    requestChain = pending.catch(() => {});
    return pending;
  }

  function reportRequest(method, params) {
    const paneId = process.env.HERDR_PANE_ID;
    if (!paneId) {
      return Promise.resolve();
    }
    return request(method, {
      pane_id: paneId,
      source: SOURCE,
      agent: AGENT,
      seq: nextReportSeq(),
      ...params,
    });
  }

  function reportSession(sessionID, sessionStartSource) {
    if (disposed || !sessionID) {
      return Promise.resolve();
    }
    const params = { agent_session_id: sessionID };
    if (sessionStartSource) {
      params.session_start_source = sessionStartSource;
    }
    return reportRequest("pane.report_agent_session", params);
  }

  function reportState(state, sessionID, suppressCompletion = false) {
    if (disposed) {
      return Promise.resolve();
    }
    const params = { state };
    if (sessionID) {
      params.agent_session_id = sessionID;
    }
    if (suppressCompletion) {
      params.suppress_completion = true;
    }
    return reportRequest("pane.report_agent", params);
  }

  function responseResult(response, expectedType) {
    const result = response?.result;
    return result?.type === expectedType ? result : undefined;
  }

  function responseErrorCode(response) {
    return typeof response?.error?.code === "string" ? response.error.code : undefined;
  }

  async function serverAcceptsAttach() {
    if (!attachServerUrl) {
      return false;
    }
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), SERVER_PROBE_TIMEOUT_MS);
    timeout.unref?.();
    try {
      const response = await fetch(new URL("/global/health", attachServerUrl), {
        signal: controller.signal,
      });
      await response.body?.cancel();
      return true;
    } catch {
      return false;
    } finally {
      clearTimeout(timeout);
    }
  }

  function subagentName(sessionID) {
    const suffix = createHash("sha256").update(sessionID).digest("hex").slice(0, 12);
    return `opencode-${suffix}`;
  }

  function childDirectory(info) {
    return typeof info?.directory === "string" && info.directory
      ? info.directory
      : defaultDirectory;
  }

  function splitTarget(layout) {
    const rootPaneID = process.env.HERDR_PANE_ID;
    if (!rootPaneID) {
      return undefined;
    }
    const panes = Array.isArray(layout?.panes) ? layout.panes : [];
    const childPaneIDs = new Set(
      [...children.values()].map((child) => child.paneID).filter(Boolean),
    );
    let candidates = panes.filter((pane) => childPaneIDs.has(pane?.pane_id));
    // The first child splits the root to establish a permanent left Main column.
    // Later children divide only the largest area in the right child subtree.
    const splitRoot = candidates.length === 0;
    if (splitRoot) {
      candidates = panes.filter((pane) => pane?.pane_id === rootPaneID);
    }
    let targetPaneID;
    let rect;
    for (const candidate of candidates) {
      const candidateRect = candidate?.rect;
      if (
        typeof candidateRect?.width !== "number" ||
        typeof candidateRect?.height !== "number"
      ) {
        continue;
      }
      const candidateArea = candidateRect.width * candidateRect.height;
      const selectedArea = rect ? rect.width * rect.height : -1;
      if (candidateArea > selectedArea) {
        targetPaneID = candidate.pane_id;
        rect = candidateRect;
      }
    }
    if (!targetPaneID) {
      return undefined;
    }
    return {
      paneID: targetPaneID,
      direction:
        splitRoot || rect.width >= MIN_READABLE_SUBAGENT_COLUMNS * 2 ? "right" : "down",
    };
  }

  async function childSessionState(sessionID) {
    if (typeof client?.session?.status !== "function") {
      return "idle";
    }
    try {
      const response = await client.session.status();
      const statuses = response?.data;
      if (!statuses || typeof statuses !== "object") {
        return undefined;
      }
      const status = statuses[sessionID];
      return status === undefined ? "idle" : stateFromSessionStatus(status);
    } catch {
      return undefined;
    }
  }

  async function closeChildPane(sessionID, allowWhileDisposing = false) {
    const child = children.get(sessionID);
    const paneID = child?.paneID;
    if (!paneID) {
      return true;
    }
    const response = await request("pane.close", { pane_id: paneID }, allowWhileDisposing);
    let closed = Boolean(responseResult(response, "ok")) || responseErrorCode(response) === "pane_not_found";
    if (!closed) {
      const layoutResponse = await request(
        "pane.layout",
        { pane_id: paneID },
        allowWhileDisposing,
      );
      closed = responseErrorCode(layoutResponse) === "pane_not_found";
    }
    if (closed && child.paneID === paneID) {
      child.paneID = undefined;
    }
    return closed;
  }

  async function openChildPane(sessionID) {
    const child = children.get(sessionID);
    const info = child?.info;
    const rootPaneID = process.env.HERDR_PANE_ID;
    if (
      disposed ||
      disposing ||
      !attachServerUrl ||
      !rootPaneID ||
      typeof sessionID !== "string" ||
      !sessionID ||
      !child ||
      !info ||
      info.parentID !== currentRootSessionID ||
      retiredChildren.has(sessionID) ||
      !child.working ||
      child.paneID ||
      child.spawning
    ) {
      return;
    }

    child.spawning = true;
    try {
      if (!(await serverAcceptsAttach()) || disposing || retiredChildren.has(sessionID)) {
        return;
      }

      const previousPanePlacement = panePlacementChain;
      let releasePanePlacement = () => {};
      panePlacementChain = new Promise((resolve) => {
        releasePanePlacement = resolve;
      });
      await previousPanePlacement;

      let directory;
      let paneID;
      try {
        if (
          disposed ||
          disposing ||
          !child.working ||
          child.paneID ||
          retiredChildren.has(sessionID) ||
          info.parentID !== currentRootSessionID
        ) {
          return;
        }
        const layoutResponse = await request("pane.layout", { pane_id: rootPaneID });
        const layout = responseResult(layoutResponse, "pane_layout")?.layout;
        const target = splitTarget(layout);
        if (
          disposing ||
          !target ||
          !child.working ||
          retiredChildren.has(sessionID)
        ) {
          return;
        }

        directory = childDirectory(info);
        const splitResponse = await request("pane.split", {
          target_pane_id: target.paneID,
          direction: target.direction,
          ratio: 0.5,
          ...(directory ? { cwd: directory } : {}),
          focus: false,
          env: { [SUBAGENT_SESSION_ENV]: sessionID },
        });
        paneID = responseResult(splitResponse, "pane_info")?.pane?.pane_id;
        if (typeof paneID !== "string" || !paneID) {
          return;
        }
        child.paneID = paneID;
        if (disposing || !child.working || retiredChildren.has(sessionID)) {
          await closeChildPane(sessionID, disposing);
          return;
        }
      } finally {
        releasePanePlacement();
      }

      const startResponse = await request("agent.start", {
        name: subagentName(sessionID),
        kind: AGENT,
        pane_id: paneID,
        args: [
          "attach",
          attachServerUrl,
          "--session",
          sessionID,
          ...(directory ? ["--dir", directory] : []),
        ],
        timeout_ms: SUBAGENT_START_TIMEOUT_MS,
      });
      if (!responseResult(startResponse, "agent_started")) {
        await closeChildPane(sessionID, disposing);
      }
    } finally {
      child.spawning = false;
    }
  }

  function reconcileChildPane(sessionID, allowWhileDisposing = false) {
    const child = children.get(sessionID);
    if (!child) {
      return Promise.resolve();
    }
    const previous = child.reconcileChain ?? Promise.resolve();
    const pending = previous.then(async () => {
      if (children.get(sessionID) !== child) {
        return;
      }
      if (child.working && !disposing && !retiredChildren.has(sessionID)) {
        if (!child.paneID) {
          await openChildPane(sessionID);
        }
        return;
      }
      if (!child.paneID) {
        return;
      }
      if (!disposing && !retiredChildren.has(sessionID)) {
        const liveState = await childSessionState(sessionID);
        if (child.working) {
          return;
        }
        if (liveState === "working") {
          child.working = true;
          return;
        }
        if (liveState !== "idle") {
          return;
        }
      }
      await closeChildPane(sessionID, allowWhileDisposing || disposing);
    });
    child.reconcileChain = pending.catch(() => {});
    return pending;
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

  async function retireChild(sessionID) {
    if (!sessionID) {
      return;
    }
    clearPromptsForSession(sessionID);
    retiredChildren.add(sessionID);
    const child = children.get(sessionID);
    if (!child) {
      return;
    }
    child.working = false;
    await reconcileChildPane(sessionID);
    if (!child.paneID && children.get(sessionID) === child) {
      children.delete(sessionID);
    }
  }

  function retireChildrenOutsideRoot(rootSessionID) {
    for (const [childSessionID, child] of children) {
      if (child.info.parentID !== rootSessionID) {
        void retireChild(childSessionID);
      }
    }
  }

  function retireAllChildren() {
    for (const childSessionID of [...children.keys()]) {
      void retireChild(childSessionID);
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

  async function reportIdleOrConfirmError(sessionID, suppressCompletion = false) {
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
    await reportState("idle", sessionID, suppressCompletion);
  }

  async function dispose() {
    if (disposed || disposing) {
      return;
    }
    disposing = true;
    for (const child of children.values()) {
      child.working = false;
    }
    await Promise.all(
      [...children.keys()].map((sessionID) => reconcileChildPane(sessionID, true)),
    );
    for (const finish of [...activeClients.values()]) {
      finish();
    }
    activeClients.clear();
    disposed = true;
    for (const lifecycle of sessionLifecycle.values()) {
      clearPendingError(lifecycle);
    }
    sessionLifecycle.clear();
    activePrompts.clear();
    children.clear();
    retiredChildren.clear();
    serverCreatedSessions.clear();
    currentRootSessionID = undefined;
    unscopedErrorBlocked = false;
    requestChain = Promise.resolve();
  }

  return {
    "chat.message": async ({ sessionID }) => {
      if (
        disposed || disposing ||
        (sessionID &&
          (retiredChildren.has(sessionID) || children.has(sessionID)))
      ) {
        return;
      }
      establishRootSession(sessionID);
      if (!unscopedErrorBlocked && activePrompts.size === 0) {
        await reportState("working", sessionID);
      }
    },
    event: async ({ event }) => {
      if (disposed || disposing) {
        return;
      }
      const type = event?.type;
      const properties = event?.properties ?? {};
      const sessionID = sessionIDFromProperties(properties);
      const info = properties.info;

      if (type === "session.deleted" && info?.id && info.parentID) {
        await retireChild(info.id);
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
          const child = children.get(info.id) ?? {
            info,
            working: false,
            spawning: false,
            paneID: undefined,
            reconcileChain: Promise.resolve(),
          };
          child.info = info;
          children.set(info.id, child);
          if (type === "session.created" && info.parentID === currentRootSessionID) {
            child.working = true;
            await reconcileChildPane(info.id);
          }
        } else {
          await retireChild(info.id);
        }
        return;
      }

      if (sessionID && children.has(sessionID)) {
        if (type === "session.deleted") {
          await retireChild(sessionID);
          return;
        }
        const childStatus = type === "session.status"
          ? stateFromSessionStatus(properties.status)
          : undefined;
        const child = children.get(sessionID);
        if (childStatus === "working") {
          child.working = true;
          await reconcileChildPane(sessionID);
        } else if (childStatus === "idle" || type === "session.idle") {
          child.working = false;
          await reconcileChildPane(sessionID);
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
          if (isSessionAbort(properties.error)) {
            clearSessionLifecycle(sessionID);
            if (sessionID) {
              await reportIdleOrConfirmError(sessionID, true);
            }
            break;
          }
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
