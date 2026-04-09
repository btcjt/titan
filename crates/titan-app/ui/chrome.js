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
const panelInfo = document.getElementById("panel-info");
const infoContent = document.getElementById("info-content");
const panelSigner = document.getElementById("panel-signer");
const signerContent = document.getElementById("signer-content");
const panelConsole = document.getElementById("panel-console");
const bookmarksList = document.getElementById("bookmarks-list");
const bookmarksEmpty = document.getElementById("bookmarks-empty");
const consoleLog = document.getElementById("console-log");
const consoleInput = document.getElementById("console-input");
const panelSettings = document.getElementById("panel-settings");
const tabList = document.getElementById("tab-list");

// ── State ──

let currentUrl = "";
let suppressNextPageLoad = false;
const CHROME_HEIGHT = 82; // tab strip (32) + toolbar (48) + loading bar (1) + 1px buffer

function computeContentTop() {
  const banner = document.getElementById("update-banner");
  const bannerHeight = banner && banner.offsetParent !== null ? banner.offsetHeight : 0;
  return CHROME_HEIGHT + bannerHeight;
}
const PANEL_WIDTH = 280;
let activePanel = null;
let tabs = [];
let activeTabId = null;

// ── Content Webview Layout ──

async function updateContentLayout() {
  const rightOffset = activePanel ? PANEL_WIDTH : 0;
  await invoke("resize_content", { top: computeContentTop(), right: rightOffset });
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
  const prevHost = currentUrl.split("/")[0];
  const result = await invoke("switch_tab", { tabId });
  tabs = result.tabs;
  activeTabId = result.active_tab;
  renderTabs();
  syncAddressBarToActiveTab();
  await updateContentLayout();
  // If the host changed and the info panel is open, refresh it
  const newHost = currentUrl.split("/")[0];
  if (activePanel === "info" && prevHost !== newHost) {
    await renderSiteInfo();
  }
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

  const prevHost = currentUrl.split("/")[0];
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
    // If the host changed and the info panel is open, refresh it
    const newHost = displayUrl.split("/")[0];
    if (activePanel === "info" && prevHost !== newHost) {
      await renderSiteInfo();
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

// ── Site Info Panel ──

function renderProfileSection(profile) {
  if (!profile) return "";
  const displayName = profile.display_name || profile.name;
  const parts = [];
  if (profile.picture) {
    parts.push(`<div class="info-avatar"><img src="${escapeAttr(profile.picture)}" alt="" onerror="this.style.display='none'"></div>`);
  }
  if (displayName) {
    parts.push(`<div class="info-section"><div class="info-label">Name</div><div class="info-value">${escapeHtml(displayName)}</div></div>`);
  }
  if (profile.nip05) {
    parts.push(`<div class="info-section"><div class="info-label">NIP-05</div><div class="info-value mono small">${escapeHtml(profile.nip05)}</div></div>`);
  }
  if (profile.about) {
    parts.push(`<div class="info-section"><div class="info-label">About</div><div class="info-value" style="white-space:pre-wrap;">${escapeHtml(profile.about)}</div></div>`);
  }
  if (profile.website) {
    parts.push(`<div class="info-section"><div class="info-label">Website</div><div class="info-value small"><a href="${escapeAttr(profile.website)}" target="_blank" rel="noopener">${escapeHtml(profile.website)}</a></div></div>`);
  }
  if (profile.lud16) {
    parts.push(`<div class="info-section"><div class="info-label">Lightning</div><div class="info-value mono small">${escapeHtml(profile.lud16)}</div></div>`);
  }
  if (profile.updated_at) {
    const date = new Date(profile.updated_at * 1000);
    parts.push(`<div class="info-section"><div class="info-label">Profile updated</div><div class="info-value small">${date.toLocaleDateString()} ${date.toLocaleTimeString()}</div></div>`);
  }
  return parts.join("");
}

function renderRelaysSection(relays) {
  if (!relays || relays.length === 0) return "";
  const items = relays.map((r) => {
    const markerLabel = r.marker === "write" ? "write" : r.marker === "read" ? "read" : "r/w";
    return `<div class="info-relay">
      <span class="info-relay-marker marker-${r.marker}">${markerLabel}</span>
      <span class="info-relay-url">${escapeHtml(r.url)}</span>
    </div>`;
  }).join("");
  return `
    <div class="info-section">
      <div class="info-label">Relays (NIP-65)</div>
      <div class="info-relays">${items}</div>
    </div>
  `;
}

async function renderSiteInfo() {
  infoContent.innerHTML = `<div class="info-loading">Loading...</div>`;
  try {
    const info = await invoke("get_site_info", { url: currentUrl });
    if (info.kind === "internal") {
      infoContent.innerHTML = `<div class="info-empty">Internal page &mdash; no site info.</div>`;
      return;
    }

    let html = "";

    if (info.kind === "name") {
      const utxoShort = info.owner_txid.slice(0, 12) + "..." + info.owner_txid.slice(-8);
      const txShort = info.txid.slice(0, 12) + "..." + info.txid.slice(-8);
      html += `
        <div class="info-section">
          <div class="info-label">Bitcoin Name</div>
          <div class="info-value" style="color:var(--amber);font-size:16px;">${escapeHtml(info.name)}</div>
        </div>
        ${renderProfileSection(info.profile)}
        <div class="info-section">
          <div class="info-label">Resolves to</div>
          <div class="info-value mono small">${escapeHtml(info.npub)}</div>
        </div>
        <div class="info-section">
          <div class="info-label">Owner UTXO</div>
          <div class="info-value mono small">
            <a href="https://mempool.space/tx/${info.owner_txid}" target="_blank" rel="noopener">
              ${escapeHtml(utxoShort)}:${info.owner_vout}
            </a>
          </div>
        </div>
        <div class="info-section">
          <div class="info-label">Last action tx</div>
          <div class="info-value mono small">
            <a href="https://mempool.space/tx/${info.txid}" target="_blank" rel="noopener">
              ${escapeHtml(txShort)}
            </a>
          </div>
        </div>
        <div class="info-section">
          <div class="info-label">Registered at block</div>
          <div class="info-value mono">${info.block_height.toLocaleString()}</div>
        </div>
        ${renderRelaysSection(info.relays)}
        <div class="info-section">
          <a class="info-link" href="#" data-nav="${escapeAttr(info.name)}">View full details &rarr;</a>
        </div>
      `;
    } else if (info.kind === "npub") {
      html += `
        ${renderProfileSection(info.profile)}
        <div class="info-section">
          <div class="info-label">npub</div>
          <div class="info-value mono small">${escapeHtml(info.npub)}</div>
        </div>
        <div class="info-section">
          <div class="info-label">Pubkey (hex)</div>
          <div class="info-value mono small">${escapeHtml(info.pubkey)}</div>
        </div>
        ${renderRelaysSection(info.relays)}
        <div class="info-section">
          <div class="info-note">Direct npub reference. No Bitcoin name is registered.</div>
        </div>
      `;
    }

    infoContent.innerHTML = html;
    infoContent.querySelector("[data-nav]")?.addEventListener("click", (e) => {
      e.preventDefault();
      const name = e.target.dataset.nav;
      navigate(`titan/name?q=${name}`);
      closePanel();
    });
  } catch (err) {
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
    infoContent.innerHTML = `<div class="info-empty">${escapeHtml(msg)}</div>`;
  }
}

// ── Signer Approval Modal ──

const signerModalBackdrop = document.getElementById("signer-modal-backdrop");

// Human-readable labels for sensitive event kinds
const SENSITIVE_KIND_WARNINGS = {
  0: "This event updates your profile metadata (kind 0). The site will replace your public profile.",
  3: "This event updates your contact list (kind 3). The site will replace who you follow.",
  5: "This is an event deletion request (kind 5). The site wants to delete past events.",
  10000: "This updates your mute list (kind 10000).",
  10002: "This updates your relay list (kind 10002). The site will change where your posts are discoverable.",
  10063: "This updates your Blossom server list (kind 10063).",
};

const KIND_NAMES = {
  0: "metadata",
  1: "short text note",
  3: "contact list",
  4: "encrypted direct message (deprecated)",
  5: "delete",
  6: "repost",
  7: "reaction",
  40: "channel creation",
  1984: "reporting",
  9734: "zap request",
  9735: "zap receipt",
  10000: "mute list",
  10002: "relay list",
  10063: "blossom server list",
  15128: "nsite root manifest",
  30023: "long-form content",
  35128: "nsite named manifest",
};

let currentPrompt = null; // the request being shown in the modal

async function refreshPendingPrompts() {
  try {
    const pending = await invoke("signer_pending_prompts");
    if (pending.length === 0) {
      hideSignerModal();
      return;
    }
    // Show the first pending prompt
    currentPrompt = pending[0];
    showSignerModal(currentPrompt, pending.length);
  } catch (err) {
    log("error", "failed to fetch signer prompts: " + err);
  }
}

function showSignerModal(prompt, queueCount) {
  const siteEl = document.getElementById("signer-modal-site-value");
  const methodEl = document.getElementById("signer-modal-method-value");
  const kindRow = document.getElementById("signer-modal-kind-row");
  const kindEl = document.getElementById("signer-modal-kind-value");
  const warningEl = document.getElementById("signer-modal-warning");
  const contentRow = document.getElementById("signer-modal-content-row");
  const contentEl = document.getElementById("signer-modal-content-value");
  const tagsRow = document.getElementById("signer-modal-tags-row");
  const tagsEl = document.getElementById("signer-modal-tags-value");
  const pubkeyRow = document.getElementById("signer-modal-pubkey-row");
  const pubkeyEl = document.getElementById("signer-modal-pubkey-value");
  const rawWrap = document.getElementById("signer-modal-raw-wrap");
  const rawEl = document.getElementById("signer-modal-raw-value");
  const queueEl = document.getElementById("signer-modal-queue");

  siteEl.textContent = prompt.site || "unknown";
  methodEl.textContent = prompt.method;

  // Reset optional rows
  kindRow.style.display = "none";
  warningEl.style.display = "none";
  contentRow.style.display = "none";
  tagsRow.style.display = "none";
  pubkeyRow.style.display = "none";
  rawWrap.style.display = "none";

  if (prompt.method === "signEvent") {
    const event = prompt.params || {};
    const kind = typeof event.kind === "number" ? event.kind : null;
    if (kind !== null) {
      kindRow.style.display = "block";
      const name = KIND_NAMES[kind] || "unknown";
      kindEl.textContent = `${kind} (${name})`;
      if (SENSITIVE_KIND_WARNINGS[kind]) {
        warningEl.textContent = SENSITIVE_KIND_WARNINGS[kind];
        warningEl.style.display = "block";
      }
    }
    if (typeof event.content === "string") {
      contentRow.style.display = "block";
      contentEl.textContent = event.content;
    }
    if (Array.isArray(event.tags) && event.tags.length > 0) {
      tagsRow.style.display = "block";
      tagsEl.textContent = event.tags.map((t) => JSON.stringify(t)).join("\n");
    }
    rawWrap.style.display = "block";
    rawEl.textContent = JSON.stringify(event, null, 2);

    // created_at sanity check
    if (typeof event.created_at === "number") {
      const now = Math.floor(Date.now() / 1000);
      const skew = event.created_at - now;
      if (Math.abs(skew) > 86400) {
        const direction = skew > 0 ? "in the future" : "in the past";
        const days = Math.round(Math.abs(skew) / 86400);
        warningEl.textContent =
          (warningEl.textContent ? warningEl.textContent + " " : "") +
          `This event's created_at is ${days} day(s) ${direction}.`;
        warningEl.style.display = "block";
      }
    }
  } else if (prompt.method.startsWith("nip04.") || prompt.method.startsWith("nip44.")) {
    const params = prompt.params || {};
    if (params.pubkey) {
      pubkeyRow.style.display = "block";
      pubkeyEl.textContent = params.pubkey;
    }
    rawWrap.style.display = "block";
    rawEl.textContent = JSON.stringify(params, null, 2);
    if (prompt.method.startsWith("nip04.")) {
      warningEl.textContent = "NIP-04 is deprecated. Prefer NIP-44 where possible.";
      warningEl.style.display = "block";
    }
  }

  if (queueCount > 1) {
    queueEl.style.display = "block";
    queueEl.textContent = `1 of ${queueCount}`;
  } else {
    queueEl.style.display = "none";
  }

  signerModalBackdrop.style.display = "flex";
  // Focus the approve button so Enter works immediately
  setTimeout(() => document.getElementById("signer-modal-approve").focus(), 0);
}

function hideSignerModal() {
  signerModalBackdrop.style.display = "none";
  currentPrompt = null;
}

async function resolveSignerPrompt(approved, scopeOverride) {
  if (!currentPrompt) return;
  const scope = scopeOverride || document.getElementById("signer-modal-scope").value;
  const id = currentPrompt.id;
  try {
    await invoke("signer_resolve_prompt", {
      resolution: { id, approved, scope },
    });
    log("info", `signer: ${approved ? "approved" : "denied"} ${currentPrompt.method} for ${currentPrompt.site}`);
  } catch (err) {
    log("error", "failed to resolve prompt: " + err);
  }
  // Check for more pending prompts
  await refreshPendingPrompts();
}

document.getElementById("signer-modal-approve").addEventListener("click", () => resolveSignerPrompt(true));
document.getElementById("signer-modal-deny").addEventListener("click", () => resolveSignerPrompt(false, "allow_once"));
document.getElementById("signer-modal-deny-always").addEventListener("click", () => resolveSignerPrompt(false, "deny_always"));

// Keyboard shortcuts inside the modal
document.addEventListener("keydown", (e) => {
  if (signerModalBackdrop.style.display !== "flex") return;
  if (e.key === "Enter") {
    e.preventDefault();
    e.stopImmediatePropagation();
    resolveSignerPrompt(true);
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopImmediatePropagation();
    resolveSignerPrompt(false, "allow_once");
  }
}, true); // capture phase so it runs before other handlers

listen("signer-prompt", () => {
  refreshPendingPrompts();
});

// ── Updater ──

let pendingUpdate = null;

async function checkForUpdate(manual) {
  const statusEl = document.getElementById("settings-update-status");
  if (statusEl && manual) {
    statusEl.textContent = "Checking...";
  }
  try {
    const info = await invoke("check_for_update");
    if (info.available) {
      pendingUpdate = info;
      showUpdateBanner(info);
      if (statusEl) {
        statusEl.innerHTML = `Update available: <span style="color:var(--amber);">${escapeHtml(info.new_version)}</span>`;
      }
      log("info", `update available: ${info.new_version}`);
    } else {
      pendingUpdate = null;
      hideUpdateBanner();
      if (statusEl) {
        statusEl.textContent = `Up to date (v${info.current_version})`;
      }
      if (manual) log("info", `already on latest version (${info.current_version})`);
    }
  } catch (err) {
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
    if (statusEl) {
      statusEl.textContent = "Check failed: " + msg;
    }
    log("error", "update check failed: " + msg);
  }
}

function showUpdateBanner(info) {
  const banner = document.getElementById("update-banner");
  const text = document.getElementById("update-banner-text");
  text.textContent = `Titan ${info.new_version} is available`;
  banner.style.display = "flex";
  updateContentLayout();
}

function hideUpdateBanner() {
  const banner = document.getElementById("update-banner");
  banner.style.display = "none";
  updateContentLayout();
}

async function installPendingUpdate() {
  log("info", "install button clicked");
  if (!pendingUpdate) {
    log("warn", "installPendingUpdate: no pendingUpdate in JS state");
    return;
  }
  log("info", `installing Titan ${pendingUpdate.new_version}...`);
  try {
    await invoke("install_update");
    log("info", "install_update returned normally (restart should have happened)");
  } catch (err) {
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
    log("error", "install failed: " + msg);
  }
}

document.getElementById("update-banner-install").addEventListener("click", installPendingUpdate);
document.getElementById("update-banner-dismiss").addEventListener("click", hideUpdateBanner);
document.getElementById("settings-check-update").addEventListener("click", () => checkForUpdate(true));

// ── Signer Panel ──

async function renderSignerPanel() {
  signerContent.innerHTML = `<div class="info-loading">Loading...</div>`;
  let status;
  try {
    status = await invoke("signer_status");
  } catch (err) {
    signerContent.innerHTML = `<div class="info-empty">${escapeHtml(String(err))}</div>`;
    return;
  }

  if (!status.has_identity) {
    renderSignerNotConfigured();
    return;
  }
  if (!status.unlocked) {
    renderSignerLocked();
    return;
  }
  renderSignerUnlocked(status.pubkey);
}

function renderSignerNotConfigured() {
  signerContent.innerHTML = `
    <div class="info-section">
      <div class="info-label">No identity configured</div>
      <div class="info-value small" style="color:var(--text-secondary);margin-top:4px;">
        Titan has a built-in Nostr signer. Create a new identity or import an
        existing one to get started.
      </div>
    </div>
    <div class="info-section">
      <button class="signer-btn signer-btn-primary" id="btn-signer-create">Create new identity</button>
      <button class="signer-btn" id="btn-signer-import-toggle">Import existing nsec</button>
    </div>
    <div class="info-section" id="signer-import-section" style="display:none;">
      <div class="info-label">Paste nsec or hex</div>
      <input type="password" class="signer-input" id="signer-import-input" placeholder="nsec1... or 64-char hex" spellcheck="false" autocomplete="off">
      <div class="signer-actions">
        <button class="signer-btn signer-btn-primary" id="btn-signer-import">Import</button>
        <button class="signer-btn signer-btn-secondary" id="btn-signer-import-cancel">Cancel</button>
      </div>
    </div>
  `;

  document.getElementById("btn-signer-create").addEventListener("click", async () => {
    if (!confirm("Generate a new Nostr identity?\n\nYou should back up your nsec after creation by clicking 'Reveal nsec'.")) return;
    try {
      await invoke("signer_create");
      log("info", "signer: created new identity");
      renderSignerPanel();
    } catch (err) {
      alert("Failed to create identity: " + err);
    }
  });

  const importSection = document.getElementById("signer-import-section");
  document.getElementById("btn-signer-import-toggle").addEventListener("click", () => {
    importSection.style.display = "block";
    document.getElementById("signer-import-input").focus();
  });
  document.getElementById("btn-signer-import-cancel").addEventListener("click", () => {
    importSection.style.display = "none";
    document.getElementById("signer-import-input").value = "";
  });
  document.getElementById("btn-signer-import").addEventListener("click", async () => {
    const input = document.getElementById("signer-import-input");
    const secret = input.value.trim();
    if (!secret) return;
    try {
      await invoke("signer_import", { secret });
      input.value = "";
      log("info", "signer: imported identity");
      renderSignerPanel();
    } catch (err) {
      alert("Failed to import: " + err);
    }
  });
  document.getElementById("signer-import-input").addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") document.getElementById("btn-signer-import").click();
  });
}

function renderSignerLocked() {
  signerContent.innerHTML = `
    <div class="info-section">
      <div class="info-label">Signer locked</div>
      <div class="info-value small" style="color:var(--text-secondary);margin-top:4px;">
        Your identity is stored in the system keychain. Unlock to use it for
        signing Nostr events.
      </div>
    </div>
    <div class="info-section">
      <button class="signer-btn signer-btn-primary" id="btn-signer-unlock">Unlock</button>
    </div>
  `;
  document.getElementById("btn-signer-unlock").addEventListener("click", async () => {
    try {
      await invoke("signer_unlock");
      log("info", "signer: unlocked");
      renderSignerPanel();
    } catch (err) {
      alert("Failed to unlock: " + err);
    }
  });
}

function renderSignerUnlocked(pubkeyHex) {
  signerContent.innerHTML = `
    <div class="info-section">
      <div class="info-label">Active identity</div>
      <div class="info-value" style="color:var(--amber);margin-top:2px;">Unlocked</div>
    </div>
    <div class="info-section">
      <div class="info-label">Pubkey (hex)</div>
      <div class="info-value mono small" id="signer-pubkey-hex" style="cursor:pointer;" title="Click to copy">${escapeHtml(pubkeyHex)}</div>
    </div>
    <div class="info-section">
      <button class="signer-btn" id="btn-signer-reveal">Reveal nsec</button>
      <button class="signer-btn" id="btn-signer-lock-now">Lock signer</button>
      <button class="signer-btn signer-btn-danger" id="btn-signer-delete">Delete identity</button>
    </div>
    <div class="info-section" id="signer-reveal-section" style="display:none;">
      <div class="info-label" style="color:#c88;">Your nsec (KEEP SECRET)</div>
      <div class="info-value mono small" id="signer-nsec-value" style="word-break:break-all;cursor:pointer;" title="Click to copy"></div>
      <div class="info-value small" style="margin-top:6px;color:var(--text-muted);">
        Anyone with this key can impersonate you. Back it up offline and never share it.
      </div>
      <button class="signer-btn signer-btn-secondary" id="btn-signer-reveal-close" style="margin-top:8px;">Hide</button>
    </div>
    <div class="info-section">
      <div class="info-label">Site permissions</div>
      <div id="signer-permissions-list" style="margin-top:6px;"></div>
    </div>
  `;

  document.getElementById("signer-pubkey-hex").addEventListener("click", () => {
    navigator.clipboard.writeText(pubkeyHex);
    log("info", "signer: copied pubkey to clipboard");
  });

  const revealSection = document.getElementById("signer-reveal-section");
  document.getElementById("btn-signer-reveal").addEventListener("click", async () => {
    if (!confirm("Reveal your nsec?\n\nAnyone who sees this key can impersonate you. Make sure nobody is looking over your shoulder.")) return;
    try {
      const nsec = await invoke("signer_reveal_nsec");
      const valueEl = document.getElementById("signer-nsec-value");
      valueEl.textContent = nsec;
      valueEl.addEventListener("click", () => {
        navigator.clipboard.writeText(nsec);
        log("info", "signer: copied nsec to clipboard");
      });
      revealSection.style.display = "block";
    } catch (err) {
      alert("Failed to reveal: " + err);
    }
  });
  document.getElementById("btn-signer-reveal-close").addEventListener("click", () => {
    revealSection.style.display = "none";
    document.getElementById("signer-nsec-value").textContent = "";
  });

  document.getElementById("btn-signer-lock-now").addEventListener("click", async () => {
    await invoke("signer_lock");
    log("info", "signer: locked");
    renderSignerPanel();
  });

  document.getElementById("btn-signer-delete").addEventListener("click", async () => {
    if (!confirm("Delete your identity?\n\nThis removes the nsec from the system keychain permanently. If you don't have a backup, you will lose access to this identity forever.\n\nContinue?")) return;
    if (!confirm("Really delete?\n\nThis cannot be undone.")) return;
    try {
      await invoke("signer_delete");
      log("warn", "signer: identity deleted");
      renderSignerPanel();
    } catch (err) {
      alert("Failed to delete: " + err);
    }
  });

  // Load and render stored site permissions
  renderSignerPermissions();
}

async function renderSignerPermissions() {
  const listEl = document.getElementById("signer-permissions-list");
  if (!listEl) return;
  try {
    const perms = await invoke("signer_list_permissions");
    if (perms.length === 0) {
      listEl.innerHTML = `<div class="info-value small" style="color:var(--text-muted);">No sites have stored permissions yet.</div>`;
      return;
    }

    // Group by site
    const bySite = {};
    for (const p of perms) {
      if (!bySite[p.site]) bySite[p.site] = [];
      bySite[p.site].push(p);
    }

    const html = Object.entries(bySite).map(([site, methods]) => {
      const items = methods.map((p) => {
        const scopeLabel = p.scope === "allow_always" ? "always allow" : "always deny";
        const scopeColor = p.scope === "allow_always" ? "var(--amber)" : "#c88";
        return `<div class="signer-perm-item">
          <div class="signer-perm-method">${escapeHtml(p.method)}</div>
          <div class="signer-perm-scope" style="color:${scopeColor};">${scopeLabel}</div>
          <button class="signer-perm-revoke" data-site="${escapeAttr(p.site)}" data-method="${escapeAttr(p.method)}" title="Revoke">&times;</button>
        </div>`;
      }).join("");
      return `<div class="signer-perm-site">
        <div class="signer-perm-site-header">
          <span class="signer-perm-site-name">${escapeHtml(site)}</span>
          <button class="signer-perm-revoke-all" data-site="${escapeAttr(site)}" title="Revoke all for this site">revoke all</button>
        </div>
        ${items}
      </div>`;
    }).join("");

    listEl.innerHTML = html;

    listEl.querySelectorAll(".signer-perm-revoke").forEach((btn) => {
      btn.addEventListener("click", async () => {
        try {
          await invoke("signer_revoke_permission", {
            site: btn.dataset.site,
            method: btn.dataset.method,
          });
          log("info", `revoked ${btn.dataset.method} for ${btn.dataset.site}`);
          renderSignerPermissions();
        } catch (err) {
          alert("Failed to revoke: " + err);
        }
      });
    });

    listEl.querySelectorAll(".signer-perm-revoke-all").forEach((btn) => {
      btn.addEventListener("click", async () => {
        if (!confirm(`Revoke all permissions for ${btn.dataset.site}?`)) return;
        try {
          await invoke("signer_revoke_site", { site: btn.dataset.site });
          log("info", `revoked all permissions for ${btn.dataset.site}`);
          renderSignerPermissions();
        } catch (err) {
          alert("Failed to revoke: " + err);
        }
      });
    });
  } catch (err) {
    listEl.innerHTML = `<div class="info-value small" style="color:#c66;">${escapeHtml(String(err))}</div>`;
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
  panelInfo.style.display = "none";
  panelSigner.style.display = "none";

  if (name === "bookmarks") {
    panelTitle.textContent = "Bookmarks";
    panelBookmarks.style.display = "block";
    await renderBookmarks();
  } else if (name === "console") {
    panelTitle.textContent = "Console";
    panelConsole.style.display = "block";
    setTimeout(() => consoleInput.focus(), 0);
  } else if (name === "settings") {
    panelTitle.textContent = "Settings";
    panelSettings.style.display = "block";
    await loadSettingsUI();
  } else if (name === "info") {
    panelTitle.textContent = "Site Info";
    panelInfo.style.display = "block";
    await renderSiteInfo();
  } else if (name === "signer") {
    panelTitle.textContent = "Signer";
    panelSigner.style.display = "block";
    await renderSignerPanel();
  }

  activePanel = name;
  sidePanel.style.display = "flex";
  document.body.classList.add("panel-open");
  document.body.classList.toggle("panel-console", name === "console");
  updatePanelButtonState();
  await updateContentLayout();
}

async function closePanel() {
  activePanel = null;
  sidePanel.style.display = "none";
  document.body.classList.remove("panel-open");
  document.body.classList.remove("panel-console");
  updatePanelButtonState();
  await updateContentLayout();
}

function updatePanelButtonState() {
  document.querySelectorAll("#toolbar button[data-panel]").forEach((btn) => {
    if (btn.dataset.panel === activePanel) {
      btn.classList.add("panel-active");
    } else {
      btn.classList.remove("panel-active");
    }
  });
}

// ── Dev Console ──

function logEvalInput(code) {
  const entry = document.createElement("div");
  entry.className = "console-entry eval-input";
  const prompt = document.createElement("span");
  prompt.className = "console-time";
  prompt.textContent = "> ";
  entry.appendChild(prompt);
  const codeEl = document.createElement("span");
  codeEl.textContent = code;
  entry.appendChild(codeEl);
  consoleLog.appendChild(entry);
  consoleLog.scrollTop = consoleLog.scrollHeight;
}

function logEvalResult(level, text) {
  const entry = document.createElement("div");
  entry.className = `console-entry eval-result ${level}`;
  const prefix = document.createElement("span");
  prefix.className = "console-time";
  prefix.textContent = level === "error" ? "!" : "\u2190";
  entry.appendChild(prefix);
  const pre = document.createElement("span");
  pre.style.whiteSpace = "pre-wrap";
  pre.textContent = " " + text;
  entry.appendChild(pre);
  consoleLog.appendChild(entry);
  consoleLog.scrollTop = consoleLog.scrollHeight;
}

// REPL history (up/down arrows)
const consoleHistory = [];
let consoleHistoryIdx = 0;

async function submitConsoleEval() {
  const code = consoleInput.value.trim();
  if (!code) return;
  logEvalInput(code);
  consoleHistory.push(code);
  if (consoleHistory.length > 100) consoleHistory.shift();
  consoleHistoryIdx = consoleHistory.length;
  consoleInput.value = "";
  try {
    await invoke("console_eval", { code });
  } catch (err) {
    logEvalResult("error", "console_eval failed: " + (err.message || String(err)));
  }
}

consoleInput.addEventListener("keydown", (e) => {
  e.stopPropagation(); // prevent Cmd+L etc. from triggering toolbar shortcuts
  if (e.key === "Enter") {
    e.preventDefault();
    submitConsoleEval();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    if (consoleHistoryIdx > 0) {
      consoleHistoryIdx -= 1;
      consoleInput.value = consoleHistory[consoleHistoryIdx] || "";
    }
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    if (consoleHistoryIdx < consoleHistory.length - 1) {
      consoleHistoryIdx += 1;
      consoleInput.value = consoleHistory[consoleHistoryIdx] || "";
    } else {
      consoleHistoryIdx = consoleHistory.length;
      consoleInput.value = "";
    }
  }
});

function logRust(level, target, msg) {
  const entry = document.createElement("div");
  entry.className = `console-entry rust ${level}`;

  const time = document.createElement("span");
  time.className = "console-time";
  time.textContent = new Date().toLocaleTimeString();
  entry.appendChild(time);

  const tag = document.createElement("span");
  tag.className = "console-rust-tag";
  tag.textContent = "[rust]";
  entry.appendChild(tag);

  const targetSpan = document.createElement("span");
  targetSpan.className = "console-rust-target";
  targetSpan.textContent = " " + target;
  entry.appendChild(targetSpan);

  const text = document.createElement("span");
  text.textContent = " " + msg;
  entry.appendChild(text);

  consoleLog.appendChild(entry);
  consoleLog.scrollTop = consoleLog.scrollHeight;
}

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
document.getElementById("btn-info").addEventListener("click", () => openPanel("info"));
document.getElementById("btn-signer").addEventListener("click", () => openPanel("signer"));
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
      const prevHost = currentUrl.split("/")[0];
      addressBar.value = url;
      currentUrl = url;
      updateStarState();
      // If the host changed and the info panel is open, refresh it
      const newHost = url.split("/")[0];
      if (activePanel === "info" && prevHost !== newHost) {
        renderSiteInfo();
      }
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

listen("console-result", (event) => {
  const { level, message } = event.payload;
  logEvalResult(level || "info", message);
});

// Rust-side tracing events forwarded from the Tauri layer
listen("rust-log", (event) => {
  const { level, target, message } = event.payload || {};
  if (!message) return;
  logRust(level || "info", target || "", message);
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

  // Check for updates in the background (non-blocking, delayed slightly
  // to avoid competing with initial page load for network)
  setTimeout(() => checkForUpdate(false), 3000);
});
