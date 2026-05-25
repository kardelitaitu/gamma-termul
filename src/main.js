const { core, event } = window.__TAURI__;

const invoke = core.invoke;
const listen = event.listen;

const state = {
  config: null,
  sessions: [],
  activeId: null,
  editingId: null,
  editingValue: "",
  outputBySession: new Map(),
  lastEventLabel: "Waiting for session activity",
  toastTimer: null,
  renameInFlight: false,
  outputLimit: 18000,
};

const els = {};
const decoder = new TextDecoder();

function boot() {
  cacheElements();
  bindControls();
  hydrate().catch((error) => {
    showToast(`Startup failed: ${formatError(error)}`, "error");
    setStatus("Unable to reach the Rust backend");
  });
}

function cacheElements() {
  els.tabStrip = document.getElementById("tab-strip");
  els.sessionDetail = document.getElementById("session-detail");
  els.sessionOutput = document.getElementById("session-output");
  els.sessionCount = document.getElementById("session-count");
  els.activeBadges = document.getElementById("active-badges");
  els.statusText = document.getElementById("status-text");
  els.configText = document.getElementById("config-text");
  els.outputNote = document.getElementById("output-note");
  els.newTab = document.getElementById("new-tab");
  els.refresh = document.getElementById("refresh");
  els.toast = document.getElementById("toast");
}

function bindControls() {
  els.newTab.addEventListener("click", createTab);
  els.refresh.addEventListener("click", hydrate);

  els.tabStrip.addEventListener("click", onTabStripClick);
  els.tabStrip.addEventListener("submit", onTabStripSubmit);
  els.tabStrip.addEventListener("keydown", onTabStripKeyDown);
  els.tabStrip.addEventListener("focusout", onTabStripFocusOut);

  listen("termul:session-event", onSessionEvent).catch((error) => {
    showToast(`Event bridge unavailable: ${formatError(error)}`, "error");
  });
}

async function hydrate() {
  const [config, sessions, active] = await Promise.all([
    invoke("load_config"),
    invoke("list_sessions"),
    invoke("active_session"),
  ]);

  state.config = config;
  state.sessions = sessions ?? [];
  state.activeId = active?.id ?? state.sessions.find((session) => session.active)?.id ?? null;
  if (state.editingId != null && !state.sessions.some((session) => session.id === state.editingId)) {
    state.editingId = null;
    state.editingValue = "";
  }

  render();
  setStatus(buildStatusText());
  setConfigLabel();
}

function render() {
  renderTabs();
  renderSessionDetail();
  renderOutput();
  renderCounts();
}

function renderTabs() {
  if (!state.sessions.length) {
    els.tabStrip.innerHTML = `
      <div class="detail-empty">
        No sessions are open. Create the first tab to start a shell.
      </div>
    `;
    return;
  }

  els.tabStrip.innerHTML = state.sessions
    .map((session) => {
      const active = session.active ? "active" : "";
      const editing = state.editingId === session.id ? "editing" : "";
      const title = escapeHtml(session.title || defaultTabTitle(session.id));
      const cwd = escapeHtml(prettyPath(session.cwd));

      return `
        <article class="tab-card ${active} ${editing}" data-session-id="${session.id}">
          <button class="tab-main" type="button" data-action="activate" ${editing ? "hidden" : ""}>
            <span class="tab-main-inner">
              <span class="tab-title">${title}</span>
              <span class="tab-path">${cwd}</span>
            </span>
          </button>
          <div class="tab-actions" ${editing ? "hidden" : ""}>
            <button class="icon-button ghost" type="button" data-action="rename">Rename</button>
            <button class="icon-button danger" type="button" data-action="close">Close</button>
          </div>
          ${
            editing
              ? `
            <form class="rename-form" data-action="rename-form">
              <label class="sr-only" for="rename-${session.id}">Rename tab</label>
              <input
                id="rename-${session.id}"
                class="rename-input"
                data-role="rename-input"
                type="text"
                value="${escapeHtml(state.editingValue)}"
                maxlength="80"
                autocomplete="off"
                spellcheck="false"
              />
              <div class="rename-actions">
                <button class="icon-button primary" type="submit">Save</button>
                <button class="icon-button ghost" type="button" data-action="cancel-rename">Cancel</button>
              </div>
            </form>
          `
              : ""
          }
        </article>
      `;
    })
    .join("");

  if (state.editingId != null) {
    requestAnimationFrame(() => {
      const input = els.tabStrip.querySelector('[data-role="rename-input"]');
      if (input) {
        input.focus();
        input.select();
      }
    });
  }
}

function renderSessionDetail() {
  const active = getActiveSession();
  if (!active) {
    els.sessionDetail.innerHTML = `
      <div class="detail-empty">
        No active session yet.
      </div>
    `;
    els.activeBadges.innerHTML = "";
    return;
  }

  const cwd = prettyPath(active.cwd);
  els.sessionDetail.innerHTML = `
    <div class="detail-grid">
      <div class="detail-card">
        <span class="detail-label">Tab title</span>
        <p class="detail-value">${escapeHtml(active.title || defaultTabTitle(active.id))}</p>
      </div>
      <div class="detail-card">
        <span class="detail-label">Working directory</span>
        <p class="detail-value">${escapeHtml(cwd)}</p>
      </div>
      <div class="detail-card">
        <span class="detail-label">Shell</span>
        <p class="detail-value">${escapeHtml(active.shell || "unknown")}</p>
      </div>
      <div class="detail-card">
        <span class="detail-label">Viewport</span>
        <p class="detail-value">${escapeHtml(`${active.cols} cols x ${active.rows} rows`)}</p>
      </div>
      <div class="detail-card">
        <span class="detail-label">Process</span>
        <p class="detail-value">${escapeHtml(active.running ? "Running" : "Stopped")}</p>
      </div>
      <div class="detail-card">
        <span class="detail-label">Restore</span>
        <p class="detail-value">${escapeHtml(buildRestoreSummary())}</p>
      </div>
    </div>
  `;

  const badges = [];
  badges.push(`<span class="badge active">Active tab #${active.id}</span>`);
  badges.push(`<span class="badge ${active.running ? "running" : ""}">${active.running ? "Running" : "Stopped"}</span>`);
  if (active.process_id != null) {
    badges.push(`<span class="badge">PID ${active.process_id}</span>`);
  }
  els.activeBadges.innerHTML = badges.join("");
}

function renderOutput() {
  const active = getActiveSession();
  if (!active) {
    els.sessionOutput.textContent = "No active session.";
    els.outputNote.textContent = "Waiting for an active tab";
    return;
  }

  const output = state.outputBySession.get(active.id) || "";
  els.sessionOutput.textContent = output || "No output yet.";
  els.outputNote.textContent = state.lastEventLabel;
  requestAnimationFrame(() => {
    els.sessionOutput.scrollTop = els.sessionOutput.scrollHeight;
  });
}

function renderCounts() {
  const total = state.sessions.length;
  const active = getActiveSession();
  els.sessionCount.textContent = `${total} tab${total === 1 ? "" : "s"} open`;
  setStatus(buildStatusText(active));
}

function buildStatusText(active = getActiveSession()) {
  if (!active) {
    return "No active session";
  }

  const cwd = prettyPath(active.cwd);
  return `Active: ${active.title || defaultTabTitle(active.id)} · ${cwd}`;
}

function setConfigLabel() {
  const version = state.config?.version ?? "unknown";
  els.configText.textContent = `gamma-termul.config · v${version}`;
}

async function createTab() {
  try {
    await invoke("create_session", { config: {} });
    state.editingId = null;
    await hydrate();
    showToast("Created a new tab", "success");
  } catch (error) {
    showToast(`Failed to open tab: ${formatError(error)}`, "error");
  }
}

async function activateTab(sessionId) {
  if (sessionId == null) {
    return;
  }

  try {
    await invoke("set_active_session", { session_id: sessionId });
    state.editingId = null;
    await hydrate();
  } catch (error) {
    showToast(`Failed to activate tab: ${formatError(error)}`, "error");
  }
}

async function closeTab(sessionId) {
  if (sessionId == null) {
    return;
  }

  try {
    await invoke("close_session", { session_id: sessionId });
    if (state.editingId === sessionId) {
      state.editingId = null;
    }
    await hydrate();
    showToast("Tab closed", "success");
  } catch (error) {
    showToast(`Failed to close tab: ${formatError(error)}`, "error");
  }
}

async function renameTab(sessionId, rawTitle) {
  if (state.renameInFlight) {
    return;
  }

  state.renameInFlight = true;
  try {
    const title = String(rawTitle ?? "");
    await invoke("rename_session", { session_id: sessionId, title });
    state.editingId = null;
    state.editingValue = "";
    await hydrate();
    showToast("Tab renamed", "success");
  } catch (error) {
    showToast(`Failed to rename tab: ${formatError(error)}`, "error");
  } finally {
    state.renameInFlight = false;
  }
}

async function onTabStripClick(event) {
  const target = event.target.closest("[data-action]");
  if (!target) {
    return;
  }

  const card = target.closest("[data-session-id]");
  const sessionId = Number(card?.dataset.sessionId);
  const action = target.dataset.action;

  if (Number.isNaN(sessionId)) {
    return;
  }

  if (action === "activate") {
    await activateTab(sessionId);
  } else if (action === "rename") {
    beginRename(sessionId);
  } else if (action === "close") {
    await closeTab(sessionId);
  } else if (action === "cancel-rename") {
    cancelRename();
  }
}

async function onTabStripSubmit(event) {
  const form = event.target.closest('[data-action="rename-form"]');
  if (!form || state.renameInFlight) {
    return;
  }

  event.preventDefault();
  const card = form.closest("[data-session-id]");
  const sessionId = Number(card?.dataset.sessionId);
  const input = form.querySelector('[data-role="rename-input"]');
  if (Number.isNaN(sessionId) || !input) {
    return;
  }

  await renameTab(sessionId, input.value);
}

function onTabStripKeyDown(event) {
  const input = event.target.closest('[data-role="rename-input"]');
  if (!input) {
    return;
  }

  if (event.key === "Escape") {
    event.preventDefault();
    cancelRename();
  }
}

function onTabStripFocusOut(event) {
  const form = event.target.closest('[data-action="rename-form"]');
  if (!form) {
    return;
  }

  const nextFocus = event.relatedTarget;
  if (nextFocus && form.contains(nextFocus)) {
    return;
  }

  const card = form.closest("[data-session-id]");
  const sessionId = Number(card?.dataset.sessionId);
  const input = form.querySelector('[data-role="rename-input"]');
  if (Number.isNaN(sessionId) || !input) {
    return;
  }

  if (input.value !== state.editingValue) {
    renameTab(sessionId, input.value);
  } else {
    cancelRename();
  }
}

function beginRename(sessionId) {
  const session = state.sessions.find((item) => item.id === sessionId);
  if (!session) {
    return;
  }

  state.editingId = sessionId;
  state.editingValue = session.title || defaultTabTitle(sessionId);
  renderTabs();
}

function cancelRename() {
  state.editingId = null;
  state.editingValue = "";
  renderTabs();
}

function onSessionEvent({ payload }) {
  if (!payload || typeof payload.kind !== "string") {
    return;
  }

  const sessionId = payload.session_id;

  if (payload.kind === "output") {
    const bytes = base64ToBytes(payload.data);
    const text = decoder.decode(bytes);
    const previous = state.outputBySession.get(sessionId) || "";
    const next = trimText(previous + text, state.outputLimit);
    state.outputBySession.set(sessionId, next);
    state.lastEventLabel = `Output from tab ${sessionId}`;
    if (getActiveSession()?.id === sessionId) {
      renderOutput();
    }
    return;
  }

  if (payload.kind === "exit") {
    state.lastEventLabel = `Tab ${sessionId} exited`;
    showToast(`Tab ${sessionId} exited`, "success");
    hydrate().catch((error) => {
      showToast(`Refresh failed: ${formatError(error)}`, "error");
    });
    return;
  }

  if (payload.kind === "error") {
    state.lastEventLabel = `Backend error on tab ${sessionId}`;
    showToast(payload.message || "Session error", "error");
    if (getActiveSession()?.id === sessionId) {
      renderOutput();
    }
  }
}

function getActiveSession() {
  return state.sessions.find((session) => session.active) || state.sessions.find((session) => session.id === state.activeId) || null;
}

function buildRestoreSummary() {
  const restoreTabs = state.config?.tabs?.restore_tabs_on_startup !== false;
  const restoreActive = state.config?.tabs?.restore_last_active_tab !== false;
  return `Tabs ${restoreTabs ? "restore" : "skip"} · active ${restoreActive ? "restore" : "skip"}`;
}

function prettyPath(path) {
  if (!path) {
    return "Using portable startup directory";
  }

  const normalized = String(path).replaceAll("\\", "/");
  const segments = normalized.split("/").filter(Boolean);
  return segments.at(-1) || normalized;
}

function defaultTabTitle(sessionId) {
  return `Tab ${sessionId}`;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function trimText(value, limit) {
  if (value.length <= limit) {
    return value;
  }

  return value.slice(value.length - limit);
}

function base64ToBytes(value) {
  const binary = atob(String(value));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

function formatError(error) {
  return error instanceof Error ? error.message : String(error);
}

function setStatus(message) {
  els.statusText.textContent = message;
}

function showToast(message, tone = "info") {
  clearTimeout(state.toastTimer);
  els.toast.hidden = false;
  els.toast.dataset.tone = tone;
  els.toast.textContent = message;
  state.toastTimer = window.setTimeout(() => {
    els.toast.hidden = true;
  }, 2200);
}

document.addEventListener("DOMContentLoaded", boot);
