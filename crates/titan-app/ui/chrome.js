// Titan browser chrome — toolbar, tabs, panels (bookmarks, dev console, settings)
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const addressBar = document.getElementById("address-bar");
const btnBack = document.getElementById("btn-back");
const btnForward = document.getElementById("btn-forward");
const btnRefresh = document.getElementById("btn-refresh");
const btnStar = document.getElementById("btn-star");
const btnBookmarks = document.getElementById("btn-bookmarks");
const loadingBar = document.getElementById("loading-bar");
const sidePanel = document.getElementById("side-panel");
const panelTitle = document.getElementById("panel-title");
const panelBookmarks = document.getElementById("panel-bookmarks");
const panelConsole = document.getElementById("panel-console");
const bookmarksList = document.getElementById("bookmarks-list");
const bookmarksEmpty = document.getElementById("bookmarks-empty");
const consoleLog = document.getElementById("console-log");
const panelSettings = document.getElementById("panel-settings");
const tabList = document.getElementById("tab-list");

// ── State ──

let currentUrl = "";
let suppressNextPageLoad = false;
const CHROME_HEIGHT = 82; // tab strip (32) + toolbar (48) + loading bar (1) + 1px buffer
const CONTENT_TOP = CHROME_HEIGHT;
const PANEL_WIDTH = 280;
let activePanel = null;
let tabs = [];
let activeTabId = null;

// ── Content Webview Layout ──

async function updateContentLayout() {
  const rightOffset = activePanel ? PANEL_WIDTH : 0;
  await invoke("resize_content", { top: CONTENT_TOP, right: rightOffset });
}

// ── Tabs ──

function renderTabs() {
  tabList.innerHTML = "";
  for (const tab of tabs) {
    const el = document.createElement("div");
    el.className = "tab" + (tab.id === activeTabId ? " active" : "");
    const name = tab.title || tab.display_url || "New Tab";
    const letter = name.charAt(0).toUpperCase() || "N";
    el.innerHTML = `
      <span class="tab-favicon">${escapeHtml(letter)}</span>
      <span class="tab-title">${escapeHtml(name)}</span>
      <span class="tab-close" title="Close">&times;</span>
    `;
    el.addEventListener("click", (e) => {
      if (!e.target.classList.contains("tab-close")) switchTab(tab.id);
    });
    el.querySelector(".tab-close").addEventListener("click", (e) => {
      e.stopPropagation();
      closeTab(tab.id);
    });
    tabList.appendChild(el);
  }
}

async function createTab() {
  const result = await invoke("create_tab");
  tabs = result.tabs;
  activeTabId = result.active_tab;
  renderTabs();
  currentUrl = "";
  addressBar.value = "";
  updateStarState();
  await updateContentLayout();
  // Navigate new tab to homepage
  const settings = await invoke("get_settings");
  navigate(settings.homepage || "titan");
}

async function closeTab(tabId) {
  if (tabs.length <= 1) return;
  const result = await invoke("close_tab", { tabId });
  tabs = result.tabs;
  activeTabId = result.active_tab;
  renderTabs();
  syncAddressBarToActiveTab();
  await updateContentLayout();
}

async function switchTab(tabId) {
  if (tabId === activeTabId) return;
  const result = await invoke("switch_tab", { tabId });
  tabs = result.tabs;
  activeTabId = result.active_tab;
  renderTabs();
  syncAddressBarToActiveTab();
  await updateContentLayout();
}

function syncAddressBarToActiveTab() {
  const tab = tabs.find(t => t.id === activeTabId);
  if (tab) {
    addressBar.value = tab.display_url || "";
    currentUrl = tab.display_url || "";
    btnBack.disabled = !currentUrl;
    updateStarState();
  }
}

// ── Navigation ──

async function navigate(input) {
  const cleaned = input.trim().replace(/^nsite:\/\//, "");
  if (!cleaned) return;

  showLoading();
  log("info", `navigating to ${cleaned}`);

  try {
    const displayUrl = await invoke("navigate", { url: cleaned });
    addressBar.value = displayUrl;
    currentUrl = displayUrl;
    suppressNextPageLoad = true;
    btnBack.disabled = false;
    updateStarState();
    hideLoading();
    // Update tab state locally
    const tab = tabs.find(t => t.id === activeTabId);
    if (tab) {
      tab.display_url = displayUrl;
      tab.title = displayUrl.split("/")[0];
      renderTabs();
    }
    log("info", `loaded ${displayUrl}`);
  } catch (err) {
    hideLoading();
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
    log("error", msg);
    try {
      await invoke("navigate", { url: "internal:error:" + encodeURIComponent(msg) });
    } catch (_) {}
  }
}

// ── Bookmarks ──

async function toggleBookmark() {
  if (!currentUrl) return;

  const bookmarked = await invoke("is_bookmarked", { url: currentUrl });
  if (bookmarked) {
    await invoke("remove_bookmark", { url: currentUrl });
    log("info", `removed bookmark: ${currentUrl}`);
  } else {
    const title = currentUrl.split("/")[0] || currentUrl;
    await invoke("add_bookmark", { url: currentUrl, title });
    log("info", `bookmarked: ${currentUrl}`);
  }
  updateStarState();
  if (activePanel === "bookmarks") await renderBookmarks();
}

async function updateStarState() {
  if (!currentUrl) {
    btnStar.innerHTML = "&#x2606;";
    btnStar.classList.remove("bookmarked");
    return;
  }
  const bookmarked = await invoke("is_bookmarked", { url: currentUrl });
  if (bookmarked) {
    btnStar.innerHTML = "&#x2605;";
    btnStar.classList.add("bookmarked");
  } else {
    btnStar.innerHTML = "&#x2606;";
    btnStar.classList.remove("bookmarked");
  }
}

async function renderBookmarks() {
  const bookmarks = await invoke("list_bookmarks");
  bookmarksList.innerHTML = "";

  if (bookmarks.length === 0) {
    bookmarksEmpty.style.display = "block";
    return;
  }

  bookmarksEmpty.style.display = "none";

  for (const b of bookmarks) {
    const url = b.url;
    const item = document.createElement("div");
    item.className = "bookmark-item";
    item.innerHTML = `
      <div class="bookmark-info">
        <input class="bookmark-title-input" type="text" value="${escapeAttr(b.title)}" spellcheck="false">
        <div class="bookmark-url">nsite://${escapeHtml(b.url)}</div>
      </div>
      <button class="bookmark-delete" title="Remove">&#x2715;</button>
    `;

    item.querySelector(".bookmark-url").addEventListener("click", () => navigate(url));

    const titleInput = item.querySelector(".bookmark-title-input");
    titleInput.addEventListener("blur", async () => {
      const newTitle = titleInput.value.trim() || url;
      if (newTitle !== b.title) {
        await invoke("rename_bookmark", { url, title: newTitle });
      }
    });
    titleInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") { e.preventDefault(); titleInput.blur(); }
      e.stopPropagation();
    });
    titleInput.addEventListener("click", (e) => e.stopPropagation());

    item.querySelector(".bookmark-delete").addEventListener("click", async (e) => {
      e.stopPropagation();
      await invoke("remove_bookmark", { url });
      await renderBookmarks();
      updateStarState();
    });

    bookmarksList.appendChild(item);
  }
}

// ── Generic Panel System ──

async function openPanel(name) {
  if (activePanel === name) {
    closePanel();
    return;
  }

  panelBookmarks.style.display = "none";
  panelConsole.style.display = "none";
  panelSettings.style.display = "none";

  if (name === "bookmarks") {
    panelTitle.textContent = "Bookmarks";
    panelBookmarks.style.display = "block";
    await renderBookmarks();
  } else if (name === "console") {
    panelTitle.textContent = "Console";
    panelConsole.style.display = "block";
  } else if (name === "settings") {
    panelTitle.textContent = "Settings";
    panelSettings.style.display = "block";
    await loadSettingsUI();
  }

  activePanel = name;
  sidePanel.style.display = "flex";
  document.body.classList.add("panel-open");
  await updateContentLayout();
}

async function closePanel() {
  activePanel = null;
  sidePanel.style.display = "none";
  document.body.classList.remove("panel-open");
  await updateContentLayout();
}

// ── Dev Console ──

function log(level, msg) {
  const entry = document.createElement("div");
  entry.className = `console-entry ${level}`;

  const time = document.createElement("span");
  time.className = "console-time";
  time.textContent = new Date().toLocaleTimeString();

  entry.appendChild(time);
  entry.appendChild(document.createTextNode(msg));
  consoleLog.appendChild(entry);
  consoleLog.scrollTop = consoleLog.scrollHeight;
}

// ── Settings ──

async function loadSettingsUI() {
  const s = await invoke("get_settings");
  document.getElementById("settings-relays").value = s.relays.join("\n");
  document.getElementById("settings-discovery").value = s.discovery_relays.join("\n");
  document.getElementById("settings-blossom").value = s.blossom_servers.join("\n");
  document.getElementById("settings-indexer").value = s.indexer_pubkey;
  document.getElementById("settings-homepage").value = s.homepage;
}

async function saveSettings() {
  const settings = {
    relays: lines("settings-relays"),
    discovery_relays: lines("settings-discovery"),
    blossom_servers: lines("settings-blossom"),
    indexer_pubkey: document.getElementById("settings-indexer").value.trim(),
    homepage: document.getElementById("settings-homepage").value.trim() || "titan",
  };
  await invoke("update_settings", { settings });
  log("info", "settings saved (restart to apply relay changes)");
}

async function resetSettings() {
  const defaults = {
    relays: ["wss://relay.westernbtc.com", "wss://relay.primal.net", "wss://relay.damus.io"],
    discovery_relays: ["wss://purplepag.es", "wss://user.kindpag.es"],
    blossom_servers: ["https://blossom.westernbtc.com"],
    indexer_pubkey: "bec1a370130fed4fb9f78f9efc725b35104d827470e75573558a87a9ac5cde44",
    homepage: "titan",
  };
  await invoke("update_settings", { settings: defaults });
  await loadSettingsUI();
  log("info", "settings reset to defaults");
}

function lines(id) {
  return document.getElementById(id).value
    .split("\n")
    .map(l => l.trim())
    .filter(l => l.length > 0);
}

// ── Event Listeners ──

addressBar.addEventListener("keydown", (e) => {
  if (e.key === "Enter") navigate(addressBar.value);
});

btnBack.addEventListener("click", () => invoke("go_back"));
btnForward.addEventListener("click", () => invoke("go_forward"));
btnRefresh.addEventListener("click", () => invoke("refresh"));
btnStar.addEventListener("click", toggleBookmark);
btnBookmarks.addEventListener("click", () => openPanel("bookmarks"));
document.getElementById("btn-settings").addEventListener("click", () => openPanel("settings"));
document.getElementById("btn-console").addEventListener("click", () => openPanel("console"));
document.getElementById("btn-new-tab").addEventListener("click", createTab);
document.getElementById("settings-save").addEventListener("click", saveSettings);
document.getElementById("settings-reset").addEventListener("click", resetSettings);

// Page loaded — update address bar if from active tab
listen("page-loaded", (event) => {
  const payload = event.payload;
  if (!payload || !payload.url) return;

  const { tab_label, url } = payload;

  // Update the tab's state regardless
  const tab = tabs.find(t => t.label === tab_label);
  if (tab) {
    tab.display_url = url;
    tab.title = url.split("/")[0];
    renderTabs();
  }

  // Only update address bar if this is the active tab
  const activeTab = tabs.find(t => t.id === activeTabId);
  if (activeTab && activeTab.label === tab_label) {
    if (suppressNextPageLoad) {
      suppressNextPageLoad = false;
    } else {
      addressBar.value = url;
      currentUrl = url;
      updateStarState();
    }
  }
  hideLoading();
  log("info", `page loaded: ${url}`);
});

// Events from content webview keyboard shortcuts
listen("open-panel", (event) => {
  if (event.payload) openPanel(event.payload);
});

listen("focus-address-bar", () => {
  addressBar.focus();
  addressBar.select();
});

listen("toggle-bookmark", () => {
  toggleBookmark();
});

listen("nsite-link-clicked", (event) => {
  if (event.payload) {
    log("info", `nsite link: ${event.payload}`);
    navigate(event.payload);
  }
});

// Console messages from content webviews
listen("console-message", (event) => {
  const { level, message } = event.payload;
  log(level || "info", message);
});

listen("new-tab", () => createTab());
listen("close-tab", () => closeTab(activeTabId));
listen("switch-tab-number", (event) => {
  const num = event.payload;
  if (num === 9 && tabs.length > 0) {
    switchTab(tabs[tabs.length - 1].id);
  } else {
    const idx = num - 1;
    if (idx >= 0 && idx < tabs.length) switchTab(tabs[idx].id);
  }
});

// Keyboard shortcuts (skip when typing in settings/inputs)
document.addEventListener("keydown", (e) => {
  const tag = (e.target.tagName || "").toLowerCase();
  if (tag === "textarea") return;
  // Cmd+L — focus address bar
  if ((e.metaKey || e.ctrlKey) && e.key === "l") {
    e.preventDefault();
    addressBar.focus();
    addressBar.select();
  }
  // Cmd+D — toggle bookmark
  if ((e.metaKey || e.ctrlKey) && e.key === "d") {
    e.preventDefault();
    toggleBookmark();
  }
  // Cmd+T — new tab
  if ((e.metaKey || e.ctrlKey) && e.key === "t") {
    e.preventDefault();
    createTab();
  }
  // Cmd+W — close tab
  if ((e.metaKey || e.ctrlKey) && e.key === "w") {
    e.preventDefault();
    if (tabs.length > 1) {
      closeTab(activeTabId);
    }
  }
  // Cmd+1-9 — switch tab
  if ((e.metaKey || e.ctrlKey) && e.key >= "1" && e.key <= "9") {
    e.preventDefault();
    const num = parseInt(e.key);
    if (num === 9 && tabs.length > 0) {
      switchTab(tabs[tabs.length - 1].id);
    } else {
      const idx = num - 1;
      if (idx < tabs.length) switchTab(tabs[idx].id);
    }
  }
  // Cmd+Option+K — dev console (Mac) / Ctrl+Shift+K (other)
  if ((e.metaKey && e.altKey && e.code === "KeyK") ||
      (e.ctrlKey && e.shiftKey && e.code === "KeyK")) {
    e.preventDefault();
    openPanel("console");
  }
  // Escape — close panel
  if (e.key === "Escape" && activePanel) {
    closePanel();
  }
});

function showLoading() {
  loadingBar.className = "loading";
}

function hideLoading() {
  loadingBar.className = "done";
  setTimeout(() => { loadingBar.className = ""; }, 500);
}

function escapeHtml(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(s) {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

// Keep content webview sized correctly on window resize
window.addEventListener("resize", () => updateContentLayout());

// ── Tab Strip Drag ──
document.getElementById("tab-strip").addEventListener("mousedown", (e) => {
  const attr = e.target.getAttribute("data-tauri-drag-region");
  if (attr !== null && attr !== "false" && e.button === 0) {
    if (e.detail === 1) {
      e.preventDefault();
      window.__TAURI_INTERNALS__.invoke("plugin:window|start_dragging");
    } else if (e.detail === 2) {
      window.__TAURI_INTERNALS__.invoke("plugin:window|internal_toggle_maximize");
    }
  }
});

// ── Startup ──
log("info", "Titan started");
updateContentLayout().then(async () => {
  const result = await invoke("get_tabs");
  tabs = result.tabs;
  activeTabId = result.active_tab;
  renderTabs();
  // Navigate first tab to homepage
  const settings = await invoke("get_settings");
  navigate(settings.homepage || "titan");
});
