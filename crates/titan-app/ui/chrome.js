// Titan browser chrome — toolbar, tabs, panels (bookmarks, dev console, settings)
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const addressBar = document.getElementById("address-bar");
const btnBack = document.getElementById("btn-back");
const btnForward = document.getElementById("btn-forward");
const btnRefresh = document.getElementById("btn-refresh");
const btnStar = document.getElementById("btn-star");
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

// Side panel width is now dynamic — the user can drag the left edge
// of the side panel to resize it (see the resize handler near the
// bottom of this file). The width is stored in the CSS variable
// `--panel-width` so all the margin-right rules on the toolbar, tab
// strip, loading bar, and update banner update atomically.
const DEFAULT_PANEL_WIDTH = 280;
const MIN_PANEL_WIDTH = 280;
const MAX_PANEL_WIDTH = 1400;
let currentPanelWidth = DEFAULT_PANEL_WIDTH;

function setPanelWidth(px) {
  const clamped = Math.min(MAX_PANEL_WIDTH, Math.max(MIN_PANEL_WIDTH, Math.round(px)));
  currentPanelWidth = clamped;
  document.documentElement.style.setProperty("--panel-width", clamped + "px");
  return clamped;
}

let activePanel = null;
let tabs = [];
let activeTabId = null;

// ── Content Webview Layout ──

async function updateContentLayout() {
  const rightOffset = activePanel ? currentPanelWidth : 0;
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
//
// Three visual zones:
//   1. Identity card — avatar, name, author, about
//   2. Freshness + provenance — published date, registration, UTXO
//   3. Infrastructure — relays, blossom, npub/hex (collapsed by default)

function formatTimestamp(ts) {
  if (!ts) return null;
  const d = new Date(ts * 1000);
  return d.toLocaleDateString() + ", " + d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

function buildIdentityCard(profile, siteName, npub) {
  const displayName = (profile && (profile.display_name || profile.name)) || siteName || null;
  const about = profile && profile.about ? profile.about : null;
  const picture = profile && profile.picture && isSafeHttpUrl(profile.picture) ? profile.picture : null;
  // For Bitcoin-name sites the author is the display_name from the
  // profile. For npub sites the npub IS the identity.
  const authorLine = displayName && siteName && displayName.toLowerCase() !== siteName.toLowerCase()
    ? `<div class="info-identity-author">by ${escapeHtml(displayName)}</div>`
    : "";

  return `<div class="info-identity">
    ${picture ? `<div class="info-identity-avatar"><img src="${escapeAttr(picture)}" alt="" onerror="this.style.display='none'"></div>` : ""}
    ${siteName
      ? `<div class="info-identity-name">${escapeHtml(siteName)}</div>${authorLine}`
      : displayName
        ? `<div class="info-identity-name">${escapeHtml(displayName)}</div>`
        : `<div class="info-identity-npub">${escapeHtml(npub || "")}</div>`
    }
    ${!siteName && npub ? `<div class="info-identity-npub">${escapeHtml(npub)}</div>` : ""}
    ${about ? `<div class="info-identity-about">${escapeHtml(about)}</div>` : ""}
  </div>`;
}

function buildMetaSection(rows) {
  // rows is an array of { label, value, mono?, html? }
  const filtered = rows.filter((r) => r.value);
  if (filtered.length === 0) return "";
  const items = filtered.map((r) => {
    const cls = r.mono ? ' class="info-meta-value mono"' : ' class="info-meta-value"';
    const val = r.html ? r.value : escapeHtml(r.value);
    return `<div class="info-meta-row">
      <span class="info-meta-label">${escapeHtml(r.label)}</span>
      <span${cls}>${val}</span>
    </div>`;
  }).join("");
  return `<div class="info-meta">${items}</div>`;
}

function buildInfraSection(relays, blossomServers, npub, pubkeyHex) {
  const parts = [];

  if (relays && relays.length > 0) {
    const items = relays.map((r) => {
      const markerLabel = r.marker === "write" ? "write" : r.marker === "read" ? "read" : "r/w";
      return `<div class="info-infra-item">
        <span class="relay-marker marker-${escapeAttr(r.marker)}">${markerLabel}</span>
        <span class="info-infra-url">${escapeHtml(r.url)}</span>
      </div>`;
    }).join("");
    parts.push(`<div class="info-infra-group">
      <div class="info-infra-group-label">Relays (${relays.length})</div>
      ${items}
    </div>`);
  }

  if (blossomServers && blossomServers.length > 0) {
    const items = blossomServers.map((url) => {
      const inner = isSafeHttpUrl(url)
        ? `<a href="${escapeAttr(url)}" target="_blank" rel="noopener noreferrer">${escapeHtml(url)}</a>`
        : escapeHtml(url);
      return `<div class="info-infra-item"><span class="info-infra-url">${inner}</span></div>`;
    }).join("");
    parts.push(`<div class="info-infra-group">
      <div class="info-infra-group-label">Blossom (${blossomServers.length})</div>
      ${items}
    </div>`);
  }

  if (npub) {
    parts.push(`<div class="info-infra-group">
      <div class="info-infra-group-label">Resolves to</div>
      <div class="info-infra-mono">${escapeHtml(npub)}</div>
    </div>`);
  }

  if (pubkeyHex) {
    parts.push(`<div class="info-infra-group">
      <div class="info-infra-group-label">Pubkey (hex)</div>
      <div class="info-infra-mono">${escapeHtml(pubkeyHex)}</div>
    </div>`);
  }

  if (parts.length === 0) return "";

  return `<details class="info-infra">
    <summary>Infrastructure</summary>
    <div class="info-infra-body">${parts.join("")}</div>
  </details>`;
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
      // Identity card
      html += buildIdentityCard(info.profile, info.name, info.npub);

      // Freshness + provenance
      const published = formatTimestamp(info.manifest_updated_at);
      const ownerTxidSafe = isHex(info.owner_txid, 64) ? info.owner_txid : "";
      const ownerVout = Number.isInteger(info.owner_vout) ? info.owner_vout : 0;
      const utxoShort = ownerTxidSafe
        ? ownerTxidSafe.slice(0, 12) + "..." + ownerTxidSafe.slice(-8)
        : "invalid";
      const utxoHtml = ownerTxidSafe
        ? `<a href="https://mempool.space/tx/${ownerTxidSafe}" target="_blank" rel="noopener noreferrer">${escapeHtml(utxoShort)}:${ownerVout}</a>`
        : escapeHtml(utxoShort);

      html += buildMetaSection([
        { label: "Published", value: published },
        { label: "Block", value: info.block_height ? info.block_height.toLocaleString() : null },
        { label: "Owner UTXO", value: utxoHtml, mono: true, html: true },
      ]);

      // Infrastructure (collapsed)
      html += buildInfraSection(info.relays, info.blossom_servers, info.npub, info.pubkey);

      // Footer link
      html += `<div class="info-footer">
        <a href="#" data-nav="${escapeAttr(info.name)}">View full details on titan &rarr;</a>
      </div>`;

    } else if (info.kind === "npub") {
      // Identity card — npub IS the identity
      html += buildIdentityCard(info.profile, null, info.npub);

      // Freshness
      const published = formatTimestamp(info.manifest_updated_at);
      html += buildMetaSection([
        { label: "Published", value: published },
      ]);

      // Infrastructure
      html += buildInfraSection(info.relays, info.blossom_servers, info.npub, info.pubkey);

      // Note
      html += `<div class="info-note">Direct npub reference. No Bitcoin name is registered.</div>`;
    }

    infoContent.innerHTML = html;
    infoContent.querySelector("[data-nav]")?.addEventListener("click", (e) => {
      e.preventDefault();
      const name = e.currentTarget.dataset.nav;
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
  // Hide the content webview so the modal isn't covered by the native
  // child webview layer stacked on top of the chrome.
  invoke("hide_content_webview").catch((e) => log("error", "hide_content_webview: " + e));
  // Focus the approve button so Enter works immediately
  setTimeout(() => document.getElementById("signer-modal-approve").focus(), 0);
}

function hideSignerModal() {
  signerModalBackdrop.style.display = "none";
  currentPrompt = null;
  // Restore the content webview to its proper size (same math as
  // updateContentLayout, which takes the active side panel into account).
  updateContentLayout().catch((e) => log("error", "updateContentLayout: " + e));
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
  try {
    const info = await invoke("check_for_update");
    if (info.available) {
      pendingUpdate = info;
      showUpdateBanner(info);
      log("info", `update available: ${info.new_version}`);
    } else {
      pendingUpdate = null;
      hideUpdateBanner();
      if (manual) log("info", `already on latest version (${info.current_version})`);
    }
  } catch (err) {
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
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
    <div class="info-section">
      <div class="info-label signer-audit-header">
        <span>Recent activity</span>
        <button class="signer-audit-clear" id="btn-signer-audit-clear" title="Clear history">clear</button>
      </div>
      <div id="signer-audit-list" style="margin-top:6px;"></div>
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

  // Load and render stored site permissions + audit log
  renderSignerPermissions();
  renderSignerAuditLog();

  document.getElementById("btn-signer-audit-clear").addEventListener("click", async () => {
    if (!confirm("Clear all signer activity history?")) return;
    try {
      await invoke("signer_clear_audit_log");
      log("info", "signer: audit log cleared");
      renderSignerAuditLog();
    } catch (err) {
      alert("Failed to clear: " + err);
    }
  });
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

const OUTCOME_LABELS = {
  approved: { text: "approved", color: "var(--amber)" },
  denied: { text: "denied", color: "#c88" },
  auto_denied: { text: "auto-denied", color: "#c88" },
  signer_locked: { text: "locked", color: "var(--text-muted)" },
  timed_out: { text: "timed out", color: "var(--text-muted)" },
  failed: { text: "failed", color: "#c66" },
};

function formatRelativeTime(unixSeconds) {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - unixSeconds;
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

async function renderSignerAuditLog() {
  const listEl = document.getElementById("signer-audit-list");
  if (!listEl) return;
  try {
    const entries = await invoke("signer_audit_log");
    if (entries.length === 0) {
      listEl.innerHTML = `<div class="info-value small" style="color:var(--text-muted);">No signer activity yet.</div>`;
      return;
    }

    // Show only the 20 most recent in the panel; the rest stay in Rust memory
    const toShow = entries.slice(0, 20);

    const html = toShow.map((entry) => {
      const outcome = OUTCOME_LABELS[entry.outcome] || { text: entry.outcome, color: "var(--text-secondary)" };
      const kindSuffix = entry.kind !== undefined && entry.kind !== null ? ` (kind ${entry.kind})` : "";
      const scopeSuffix = entry.scope ? ` · ${entry.scope.replace(/_/g, " ")}` : "";
      return `<div class="signer-audit-entry">
        <div class="signer-audit-row1">
          <span class="signer-audit-site">${escapeHtml(entry.site)}</span>
          <span class="signer-audit-time">${formatRelativeTime(entry.timestamp)}</span>
        </div>
        <div class="signer-audit-row2">
          <span class="signer-audit-method">${escapeHtml(entry.method)}${escapeHtml(kindSuffix)}</span>
          <span class="signer-audit-outcome" style="color:${outcome.color};">${outcome.text}${escapeHtml(scopeSuffix)}</span>
        </div>
      </div>`;
    }).join("");

    listEl.innerHTML = html + (entries.length > 20 ? `<div class="info-value small" style="color:var(--text-muted);margin-top:6px;">+ ${entries.length - 20} older entries</div>` : "");
  } catch (err) {
    listEl.innerHTML = `<div class="info-value small" style="color:#c66;">${escapeHtml(String(err))}</div>`;
  }
}

// ── Generic Panel System ──

const panelMenu = document.getElementById("panel-menu");

async function openPanel(name) {
  if (activePanel === name) {
    closePanel();
    return;
  }

  // Hide all panel views
  panelMenu.style.display = "none";
  panelBookmarks.style.display = "none";
  panelConsole.style.display = "none";
  panelSettings.style.display = "none";
  panelInfo.style.display = "none";
  panelSigner.style.display = "none";

  if (name === "menu") {
    panelTitle.textContent = "Menu";
    panelMenu.style.display = "block";
  } else if (name === "bookmarks") {
    panelTitle.textContent = "Bookmarks";
    panelBookmarks.style.display = "block";
    await renderBookmarks();
  } else if (name === "console") {
    panelTitle.textContent = "Developer Tools";
    panelConsole.style.display = "block";
    switchDevtoolsTab("logs");
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
  // Info button in the toolbar gets active state when the info panel is open
  const btnInfo = document.getElementById("btn-info");
  if (btnInfo) {
    btnInfo.classList.toggle("panel-active", activePanel === "info");
  }
  // Menu button gets active when the menu panel itself is open, or
  // when any panel reachable from the menu is open
  const btnMenuEl = document.getElementById("btn-menu");
  const menuPanels = ["menu", "signer", "bookmarks", "settings", "console"];
  if (btnMenuEl) {
    btnMenuEl.classList.toggle("panel-active", menuPanels.includes(activePanel));
  }
}

// ── Dev Console ──

function logEvalInput(code) {
  const entry = document.createElement("div");
  entry.className = "console-entry eval-input";
  // REPL input is always user-initiated — tag as info so it survives
  // any min-level filter except "Errors only".
  entry.dataset.level = "info";
  entry.dataset.target = "repl";
  const prompt = document.createElement("span");
  prompt.className = "console-time";
  prompt.textContent = "> ";
  entry.appendChild(prompt);
  const codeEl = document.createElement("span");
  codeEl.textContent = code;
  entry.appendChild(codeEl);
  applyLogFilterToEntry(entry);
  consoleLog.appendChild(entry);
  maybeScrollLogs();
}

function logEvalResult(level, text) {
  const entry = document.createElement("div");
  entry.className = `console-entry eval-result ${level}`;
  entry.dataset.level = level;
  entry.dataset.target = "repl";
  const prefix = document.createElement("span");
  prefix.className = "console-time";
  prefix.textContent = level === "error" ? "!" : "\u2190";
  entry.appendChild(prefix);
  const pre = document.createElement("span");
  pre.style.whiteSpace = "pre-wrap";
  pre.textContent = " " + text;
  entry.appendChild(pre);
  applyLogFilterToEntry(entry);
  consoleLog.appendChild(entry);
  maybeScrollLogs();
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
  // Stash metadata for the logs-tab filters to read. Lowercase the
  // target so filter matching is case-insensitive.
  entry.dataset.level = level;
  entry.dataset.target = (target || "").toLowerCase();

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

  applyLogFilterToEntry(entry);
  consoleLog.appendChild(entry);
  maybeScrollLogs();
}

function log(level, msg) {
  const entry = document.createElement("div");
  entry.className = `console-entry ${level}`;
  entry.dataset.level = level;
  // Chrome-side log(): no target, so leave target empty. The target
  // filter treats empty as "matches everything".
  entry.dataset.target = "";

  const time = document.createElement("span");
  time.className = "console-time";
  time.textContent = new Date().toLocaleTimeString();

  entry.appendChild(time);
  entry.appendChild(document.createTextNode(msg));
  applyLogFilterToEntry(entry);
  consoleLog.appendChild(entry);
  maybeScrollLogs();
}

// ── Log filtering ──
//
// The Logs tab has two filters: a minimum level dropdown and a text
// input matching the log source (Rust target). Filtered entries are
// hidden via `.log-hidden` rather than removed, so changing the filter
// can re-reveal them without any bookkeeping.
//
// Default: hide debug-level entries. This is a MUCH better experience
// than showing everything when `RUST_LOG=titan=debug` is set — the
// user can drop to "All" or "Debug+" from the dropdown when they
// actually want to debug the resolver.

let logFilterLevel = "info"; // min level to show: all | debug | info | warn | error
let logFilterTarget = ""; // substring to match against data-target

const LEVEL_PRIORITY = {
  debug: 10,
  info: 20,
  warn: 30,
  error: 40,
};

function levelPassesFilter(entryLevel) {
  if (logFilterLevel === "all") return true;
  const min = LEVEL_PRIORITY[logFilterLevel] ?? 0;
  const got = LEVEL_PRIORITY[entryLevel] ?? 100;
  return got >= min;
}

function targetPassesFilter(entryTarget) {
  if (!logFilterTarget) return true;
  return (entryTarget || "").includes(logFilterTarget);
}

function applyLogFilterToEntry(entry) {
  const level = entry.dataset.level || "";
  const target = entry.dataset.target || "";
  const visible = levelPassesFilter(level) && targetPassesFilter(target);
  entry.classList.toggle("log-hidden", !visible);
}

function reapplyLogFilters() {
  let hidden = 0;
  for (const entry of consoleLog.querySelectorAll(".console-entry")) {
    applyLogFilterToEntry(entry);
    if (entry.classList.contains("log-hidden")) hidden += 1;
  }
  const counter = document.getElementById("logs-hidden-count");
  if (counter) {
    counter.textContent = hidden > 0 ? `${hidden} hidden` : "";
  }
}

// Auto-scroll only if the user was already at the bottom. If they
// scrolled up to read something, a new log shouldn't yank them back
// down. 8px slack to account for fractional pixel positions.
function maybeScrollLogs() {
  const atBottom =
    consoleLog.scrollHeight - consoleLog.scrollTop - consoleLog.clientHeight < 8;
  if (atBottom) {
    consoleLog.scrollTop = consoleLog.scrollHeight;
  }
}

document.getElementById("logs-level-filter").addEventListener("change", (e) => {
  logFilterLevel = e.target.value;
  reapplyLogFilters();
});

document.getElementById("logs-target-filter").addEventListener("input", (e) => {
  logFilterTarget = (e.target.value || "").toLowerCase();
  reapplyLogFilters();
});

// ── Network Tab ──
//
// The Network tab shows every fetch / XHR / WebSocket made by the
// active content page plus every nsite-content:// and titan-nostr://
// request served by the Rust side. Data flows from the Rust devtools
// ring buffer (via the `devtools_network_snapshot` command) and is
// refreshed whenever a `devtools-network-updated` Tauri event fires.
//
// The user can toggle recording, filter by URL, clear the log, click
// any row to see headers/body, and copy a given request as a `curl`
// command for reproducing it outside the browser.

let networkEvents = []; // latest snapshot from Rust
let networkSelectedId = null; // id of the currently expanded row
let networkFilterText = ""; // lowercase filter string
let networkRefreshQueued = false; // coalesce rapid updates

// Coalesced refresh: multiple `devtools-network-updated` events arriving
// in quick succession (e.g. during a page load that fires 30 requests)
// should not trigger 30 separate snapshots. We queue a single refresh
// in the next animation frame.
function queueNetworkRefresh() {
  if (networkRefreshQueued) return;
  networkRefreshQueued = true;
  requestAnimationFrame(async () => {
    networkRefreshQueued = false;
    try {
      networkEvents = await invoke("devtools_network_snapshot");
    } catch (e) {
      log("error", "devtools_network_snapshot: " + e);
      return;
    }
    renderNetworkTable();
  });
}

function clearNetworkLog() {
  invoke("devtools_network_clear").catch((e) =>
    log("error", "devtools_network_clear: " + e),
  );
  networkEvents = [];
  networkSelectedId = null;
  renderNetworkTable();
}

function getFilteredNetworkEvents() {
  if (!networkFilterText) return networkEvents;
  return networkEvents.filter((ev) =>
    (ev.url || "").toLowerCase().includes(networkFilterText),
  );
}

function renderNetworkTable() {
  const tbody = document.getElementById("network-table-body");
  const empty = document.getElementById("network-empty");
  const stats = document.getElementById("network-stats");
  const badge = document.getElementById("devtools-network-count");
  if (!tbody) return;

  const filtered = getFilteredNetworkEvents();
  const total = networkEvents.length;
  const shown = filtered.length;

  // Badge on the tab button shows current count (for at-a-glance
  // feedback even when the tab isn't active).
  if (total > 0) {
    badge.style.display = "inline-block";
    badge.textContent = total > 999 ? "999+" : String(total);
  } else {
    badge.style.display = "none";
  }

  stats.textContent = total === shown ? `${total}` : `${shown} / ${total}`;

  if (filtered.length === 0) {
    tbody.innerHTML = "";
    empty.style.display = "block";
    empty.textContent = total === 0
      ? "No network activity yet. Navigate or interact with the page."
      : "No matches for the current filter.";
    return;
  }

  empty.style.display = "none";

  const rows = filtered.map((ev) => {
    const selected = ev.id === networkSelectedId ? " selected" : "";
    const method = escapeHtml(ev.method || "GET");
    const url = escapeHtml(ev.url || "");
    const type = escapeHtml(ev.resource_type || "other");

    // Status rendering: pending / error / OK
    let statusHtml;
    if (ev.error) {
      statusHtml = `<span class="net-status-err">${escapeHtml(String(ev.status || "err"))}</span>`;
    } else if (!ev.status || ev.status === 0) {
      statusHtml = `<span class="net-status-pending">—</span>`;
    } else {
      const cls = ev.status >= 400 ? "net-status-err" : "net-status-ok";
      statusHtml = `<span class="${cls}">${ev.status}</span>`;
    }

    const timeMs = typeof ev.duration_ms === "number" ? ev.duration_ms : null;
    const timeText = timeMs == null ? "—" : `${timeMs} ms`;
    const timeClass = timeMs != null && timeMs > 500 ? "col-time net-slow" : "col-time";

    return `<tr data-net-id="${ev.id}" class="${selected.trim()}">
      <td class="col-method"><span class="net-method ${method}">${method}</span></td>
      <td class="col-status">${statusHtml}</td>
      <td class="col-url" title="${escapeAttr(ev.url || "")}">${url}</td>
      <td class="col-type">${type}</td>
      <td class="${timeClass}">${escapeHtml(timeText)}</td>
    </tr>`;
  });

  tbody.innerHTML = rows.join("");

  for (const tr of tbody.querySelectorAll("tr")) {
    tr.addEventListener("click", () => {
      const id = Number(tr.dataset.netId);
      networkSelectedId = id;
      renderNetworkDetail(id);
      for (const row of tbody.querySelectorAll("tr")) {
        row.classList.toggle("selected", Number(row.dataset.netId) === id);
      }
    });
  }
}

function renderNetworkDetail(id) {
  const detail = document.getElementById("network-detail");
  if (!detail) return;
  const ev = networkEvents.find((e) => e.id === id);
  if (!ev) {
    detail.style.display = "none";
    return;
  }

  const formatHeaders = (list) => {
    if (!Array.isArray(list) || list.length === 0) return "(none)";
    return list
      .map(
        ([k, v]) =>
          `${escapeHtml(String(k))}: ${escapeHtml(String(v == null ? "" : v))}`,
      )
      .join("\n");
  };

  const bodySection = ev.request_body
    ? `<div class="net-detail-section">
        <div class="net-detail-label">Request body</div>
        <pre class="net-detail-headers">${escapeHtml(ev.request_body)}</pre>
      </div>`
    : "";

  const errorSection = ev.error
    ? `<div class="net-detail-section">
        <div class="net-detail-label">Error</div>
        <div class="net-detail-value" style="color:#c66;">${escapeHtml(ev.error)}</div>
      </div>`
    : "";

  detail.innerHTML = `
    <div class="net-detail-section">
      <div class="net-detail-label">General</div>
      <div class="net-detail-value">${escapeHtml(ev.method || "GET")} ${escapeHtml(ev.url || "")}</div>
      <div class="net-detail-value" style="color:var(--text-muted);">
        Status ${escapeHtml(String(ev.status || "—"))}
        · ${escapeHtml(ev.resource_type || "other")}
        · ${escapeHtml(typeof ev.duration_ms === "number" ? ev.duration_ms + " ms" : "—")}
        · source: ${escapeHtml(ev.source || "?")}
      </div>
    </div>
    <div class="net-detail-section">
      <div class="net-detail-label">Request headers</div>
      <pre class="net-detail-headers">${formatHeaders(ev.request_headers)}</pre>
    </div>
    <div class="net-detail-section">
      <div class="net-detail-label">Response headers</div>
      <pre class="net-detail-headers">${formatHeaders(ev.response_headers)}</pre>
    </div>
    ${bodySection}
    ${errorSection}
    <div class="net-detail-actions">
      <button class="net-detail-btn" id="net-copy-curl">Copy as cURL</button>
      <button class="net-detail-btn" id="net-copy-url">Copy URL</button>
      <button class="net-detail-btn net-detail-close" id="net-close-detail">Close</button>
    </div>
  `;
  detail.style.display = "block";

  document.getElementById("net-copy-curl").addEventListener("click", () => {
    const cmd = buildCurlCommand(ev);
    navigator.clipboard
      .writeText(cmd)
      .then(() => log("info", "copied curl command"))
      .catch((err) => log("error", "clipboard write failed: " + err));
  });
  document.getElementById("net-copy-url").addEventListener("click", () => {
    navigator.clipboard
      .writeText(ev.url || "")
      .then(() => log("info", "copied url"))
      .catch((err) => log("error", "clipboard write failed: " + err));
  });
  document.getElementById("net-close-detail").addEventListener("click", () => {
    networkSelectedId = null;
    detail.style.display = "none";
    for (const row of document.querySelectorAll("#network-table tbody tr")) {
      row.classList.remove("selected");
    }
  });
}

// Wire up the toolbar controls once at startup. The record checkbox
// and filter input persist across panel open/close without the rest
// of the code needing to care.
document.getElementById("network-record").addEventListener("change", (e) => {
  invoke("devtools_set_network_recording", { recording: e.target.checked }).catch(
    (err) => log("error", "set recording: " + err),
  );
});

document.getElementById("network-filter").addEventListener("input", (e) => {
  networkFilterText = (e.target.value || "").toLowerCase();
  renderNetworkTable();
});

// Listen for updates from Rust. The event fires after every recorded
// request; we coalesce into a single rAF-scheduled refresh.
listen("devtools-network-updated", () => queueNetworkRefresh());

// Initial snapshot fetch happens when the user switches to the tab
// for the first time — see switchDevtoolsTab below.

// ── Application Tab ──
//
// Shows localStorage, sessionStorage, and cookies from the active
// content webview. Data arrives asynchronously: the chrome invokes
// `devtools_read_storage`, which eval's a reader script inside the
// content webview, which reports back via a `titan-cmd://devtools-
// storage/...` URL. The navigation handler parses the payload and
// emits a `devtools-storage` Tauri event that renderApplicationTab
// awaits.

let currentApplicationSnapshot = null;

function renderApplicationTab() {
  // Kick off a read — the actual rendering happens in the
  // devtools-storage listener below. Show a pending state for snappy
  // UX since the round trip is ~1 rAF on fast machines.
  invoke("devtools_read_storage").catch((err) => {
    log("error", "devtools_read_storage: " + err);
  });
}

listen("devtools-storage", (event) => {
  currentApplicationSnapshot = event.payload || null;
  paintApplicationTables();
});

function paintApplicationTables() {
  const snap = currentApplicationSnapshot || {
    origin: "",
    local: [],
    session: [],
    cookies: [],
  };
  const originEl = document.getElementById("application-origin");
  if (originEl) {
    originEl.textContent = snap.origin || snap.href || "(no origin)";
  }

  paintStorageTable(
    document.querySelector("#application-local tbody"),
    document.querySelector('.application-empty[data-for="localStorage"]'),
    snap.local || [],
    "local",
  );
  paintStorageTable(
    document.querySelector("#application-session tbody"),
    document.querySelector('.application-empty[data-for="sessionStorage"]'),
    snap.session || [],
    "session",
  );
  paintStorageTable(
    document.querySelector("#application-cookies tbody"),
    document.querySelector('.application-empty[data-for="cookies"]'),
    snap.cookies || [],
    "cookie",
  );
}

function paintStorageTable(tbody, emptyEl, rows, kind) {
  if (!tbody) return;
  if (!rows || rows.length === 0) {
    tbody.innerHTML = "";
    if (emptyEl) emptyEl.style.display = "block";
    return;
  }
  if (emptyEl) emptyEl.style.display = "none";
  tbody.innerHTML = rows
    .map(([k, v]) => {
      const keyEsc = escapeHtml(String(k == null ? "" : k));
      const valEsc = escapeHtml(String(v == null ? "" : v));
      return `<tr data-storage-key="${escapeAttr(String(k == null ? "" : k))}">
        <td>${keyEsc}</td>
        <td>${valEsc}</td>
        <td><button class="application-row-delete" title="Delete">&times;</button></td>
      </tr>`;
    })
    .join("");

  for (const btn of tbody.querySelectorAll(".application-row-delete")) {
    btn.addEventListener("click", async (e) => {
      const tr = e.currentTarget.closest("tr");
      if (!tr) return;
      const key = tr.dataset.storageKey;
      try {
        await invoke("devtools_delete_storage_key", { kind, key });
        // Storage edits happen asynchronously via eval, so give the
        // content webview a beat to apply before re-reading.
        setTimeout(renderApplicationTab, 50);
      } catch (err) {
        log("error", "delete storage key: " + err);
      }
    });
  }
}

// Wire the toolbar: refresh + clear-all buttons
document.getElementById("application-refresh").addEventListener("click", () => {
  renderApplicationTab();
});

for (const btn of document.querySelectorAll(".application-clear-all")) {
  btn.addEventListener("click", async () => {
    const kind = btn.dataset.clear;
    if (!kind) return;
    const labels = {
      localStorage: "localStorage",
      sessionStorage: "sessionStorage",
      cookies: "cookies",
    };
    const label = labels[kind] || kind;
    if (!confirm(`Clear all ${label} for this site?`)) return;
    try {
      await invoke("devtools_clear_storage", { kind });
      setTimeout(renderApplicationTab, 50);
    } catch (err) {
      log("error", "clear storage: " + err);
    }
  });
}

// ── Dev Console Tabs ──
//
// The dev console panel has three tabs: Logs (the original JS REPL +
// tracing output), Network (captured fetch/XHR/WebSocket + internal
// protocol requests), and Application (localStorage / sessionStorage /
// cookies from the active content webview).
//
// Switching tabs just toggles visibility of the three tab bodies. The
// Clear button is context-sensitive: each tab registers its own clear
// handler, and the button calls whichever is currently active.

let activeDevtoolsTab = "logs";
const devtoolsClearHandlers = {
  logs: () => {
    consoleLog.innerHTML = "";
  },
  network: () => {
    // Set in the network section below
    if (typeof clearNetworkLog === "function") clearNetworkLog();
  },
  application: () => {
    // Refresh from the content webview — effectively a re-read
    if (typeof renderApplicationTab === "function") renderApplicationTab();
  },
};

function switchDevtoolsTab(name) {
  if (!["logs", "network", "application"].includes(name)) return;
  activeDevtoolsTab = name;

  for (const tab of document.querySelectorAll(".devtools-tab")) {
    tab.classList.toggle("active", tab.dataset.devtoolsTab === name);
  }
  document.getElementById("devtools-tab-logs").style.display =
    name === "logs" ? "flex" : "none";
  document.getElementById("devtools-tab-network").style.display =
    name === "network" ? "flex" : "none";
  document.getElementById("devtools-tab-application").style.display =
    name === "application" ? "flex" : "none";

  // Tab-specific onEnter actions
  if (name === "logs") {
    setTimeout(() => consoleInput.focus(), 0);
  } else if (name === "network") {
    // Refresh the snapshot on entry so the table reflects any events
    // recorded while the tab was hidden.
    queueNetworkRefresh();
  } else if (name === "application") {
    if (typeof renderApplicationTab === "function") renderApplicationTab();
  }
}

for (const tab of document.querySelectorAll(".devtools-tab")) {
  tab.addEventListener("click", () =>
    switchDevtoolsTab(tab.dataset.devtoolsTab),
  );
}

document.getElementById("devtools-clear").addEventListener("click", () => {
  const handler = devtoolsClearHandlers[activeDevtoolsTab];
  if (handler) handler();
});

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
btnStar.addEventListener("click", toggleBookmark);

// ── Kebab Menu ──
//
// The ⋮ button opens a "menu" panel in the side panel. Clicking a
// menu item switches to the target panel. This avoids z-order
// issues with a dropdown overlapping the content webview (which is
// a native layer above the chrome DOM).

document.getElementById("btn-menu").addEventListener("click", () => openPanel("menu"));

// Menu item click handlers — each item opens a panel or runs an
// action, replacing the menu panel with the target.
for (const item of document.querySelectorAll(".menu-item")) {
  item.addEventListener("click", () => {
    const action = item.dataset.action;
    switch (action) {
      case "signer":
        openPanel("signer");
        break;
      case "bookmarks":
        openPanel("bookmarks");
        break;
      case "info":
        openPanel("info");
        break;
      case "settings":
        openPanel("settings");
        break;
      case "console":
        openPanel("console");
        break;
      case "check-update":
        closePanel();
        checkForUpdate(true);
        break;
    }
  });
}

// Populate the dev console shortcut hint in the menu (same OS-aware
// logic as the settings panel hint).
(function setMenuConsoleShortcut() {
  const el = document.getElementById("menu-console-shortcut");
  if (!el) return;
  const uaPlatform =
    (navigator.userAgentData && navigator.userAgentData.platform) || "";
  const ua = navigator.userAgent || "";
  const isMac =
    uaPlatform.toLowerCase().includes("mac") ||
    /\bMac OS X\b|\bMacintosh\b/.test(ua);
  el.textContent = isMac ? "⌘⌥K" : "Ctrl+Shift+K";
})();
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

// Bookmarks changed on the Rust side — usually after a Nostr sync pulled
// in updates from another device, or after the legacy v0.1.4 file was
// migrated to NIP-51 kind 10003. Refresh the bookmarks panel if it's
// open and re-check the star icon for the current tab.
listen("bookmarks-changed", () => {
  if (activePanel === "bookmarks") {
    renderBookmarks();
  }
  updateStarState();
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

// Security-critical helpers (escapeHtml, escapeAttr, isSafeHttpUrl, isHex)
// are defined in helpers.js, loaded before this file in chrome.html. They
// are extracted to a separate file so they can be unit tested in Node
// without loading chrome.js's DOM-dependent top-level code.

// Keep content webview sized correctly on window resize
window.addEventListener("resize", () => updateContentLayout());

// ── Side Panel Drag-to-Resize ──
//
// The left edge of the side panel is a 6px drag handle (#side-panel-
// resize). On mousedown we capture the pointer, track mousemove to
// update the panel width live (and call updateContentLayout so the
// content webview shrinks to match), and on mouseup persist the final
// width to settings via `update_side_panel_width`.
(function wireSidePanelResize() {
  const handle = document.getElementById("side-panel-resize");
  if (!handle) return;

  let dragging = false;
  // Throttle the native webview resize calls — moving the mouse fires
  // mousemove at 60+ Hz and `resize_content` is an IPC round trip.
  // We rAF the layout updates so the panel looks live but we don't
  // hammer the Rust side.
  let pendingLayout = false;

  function onMouseMove(e) {
    if (!dragging) return;
    // The panel is right-anchored, so the new width is simply the
    // distance from the mouse to the right edge of the window.
    const newWidth = window.innerWidth - e.clientX;
    setPanelWidth(newWidth);
    if (!pendingLayout) {
      pendingLayout = true;
      requestAnimationFrame(() => {
        pendingLayout = false;
        updateContentLayout().catch((err) =>
          log("error", "resize layout: " + err),
        );
      });
    }
  }

  function onMouseUp() {
    if (!dragging) return;
    dragging = false;
    handle.classList.remove("dragging");
    document.body.classList.remove("panel-resizing");
    document.removeEventListener("mousemove", onMouseMove);
    document.removeEventListener("mouseup", onMouseUp);
    // Persist the final width. We use the dedicated
    // update_side_panel_width command so a concurrent edit from the
    // Settings panel can't clobber the width (or vice versa).
    invoke("update_side_panel_width", { width: currentPanelWidth }).catch(
      (err) => log("error", "persist panel width: " + err),
    );
  }

  handle.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    dragging = true;
    handle.classList.add("dragging");
    document.body.classList.add("panel-resizing");
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  });
})();

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
  // Load settings: apply persisted side panel width, then navigate
  // to the homepage. The width is applied via the CSS variable before
  // any panel open so the first open() uses the correct size.
  const settings = await invoke("get_settings");
  if (typeof settings.side_panel_width === "number") {
    setPanelWidth(settings.side_panel_width);
  }
  navigate(settings.homepage || "titan");

  // Check for updates in the background (non-blocking, delayed slightly
  // to avoid competing with initial page load for network)
  setTimeout(() => checkForUpdate(false), 3000);
});
