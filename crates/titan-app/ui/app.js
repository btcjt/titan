// Titan browser — frontend logic
// Communicates with the Tauri backend via invoke()

const { invoke } = window.__TAURI__.core;

const addressBar = document.getElementById("address-bar");
const btnBack = document.getElementById("btn-back");
const btnForward = document.getElementById("btn-forward");
const btnRefresh = document.getElementById("btn-refresh");
const statusText = document.getElementById("status-text");
const welcome = document.getElementById("welcome");
const pageFrame = document.getElementById("page-frame");
const loadingOverlay = document.getElementById("loading-overlay");
const loadingTextEl = document.getElementById("loading-text");
const errorOverlay = document.getElementById("error-overlay");
const errorTitle = document.getElementById("error-title");
const errorMessage = document.getElementById("error-message");
const errorDismiss = document.getElementById("error-dismiss");

// ── Navigation history ──

const history = [];
let historyIndex = -1;

function pushHistory(url) {
  if (historyIndex < history.length - 1) {
    history.splice(historyIndex + 1);
  }
  history.push(url);
  historyIndex = history.length - 1;
  updateNavButtons();
}

function updateNavButtons() {
  btnBack.disabled = historyIndex <= 0;
  btnForward.disabled = historyIndex >= history.length - 1;
}

// ── Navigation ──

async function navigate(input) {
  const cleaned = input.trim().replace(/^nsite:\/\//, "");
  if (!cleaned) return;

  const slashIdx = cleaned.indexOf("/");
  const host = slashIdx >= 0 ? cleaned.substring(0, slashIdx) : cleaned;
  const path = slashIdx >= 0 ? cleaned.substring(slashIdx) : "/";

  const fullUrl = `${host}${path === "/" ? "" : path}`;
  addressBar.value = fullUrl;
  pushHistory(fullUrl);

  await doNavigate(host, path);
}

async function doNavigate(host, path) {
  showLoading("Resolving...");

  try {
    setStatus(`Resolving ${host}...`);
    const result = await invoke("navigate", { host, path });

    // result.content_url is a nsite-content:// URL that the iframe loads
    // The custom protocol handler serves all resources through the resolver
    showPage(result.content_url);
    setStatus(`Loaded ${host}${path}`);
  } catch (err) {
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
    const { title, detail } = categorizeError(msg);
    showError(title, detail);
    setStatus("Error");
  }
}

function showPage(contentUrl) {
  hideOverlays();
  welcome.style.display = "none";
  pageFrame.style.display = "flex";
  pageFrame.src = contentUrl;
}

// ── UI state helpers ──

function showLoading(text) {
  hideOverlays();
  loadingTextEl.textContent = text || "Loading...";
  loadingOverlay.style.display = "flex";
}

function showError(title, message) {
  hideOverlays();
  errorTitle.textContent = title;
  errorMessage.textContent = message;
  errorOverlay.style.display = "flex";
}

function hideOverlays() {
  loadingOverlay.style.display = "none";
  errorOverlay.style.display = "none";
}

function setStatus(text) {
  statusText.textContent = text;
}

// ── Event listeners ──

addressBar.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    navigate(addressBar.value);
  }
});

btnBack.addEventListener("click", () => {
  if (historyIndex > 0) {
    historyIndex--;
    const url = history[historyIndex];
    addressBar.value = url;
    updateNavButtons();
    navigateFromHistory(url);
  }
});

btnForward.addEventListener("click", () => {
  if (historyIndex < history.length - 1) {
    historyIndex++;
    const url = history[historyIndex];
    addressBar.value = url;
    updateNavButtons();
    navigateFromHistory(url);
  }
});

btnRefresh.addEventListener("click", () => {
  if (history.length > 0) {
    navigateFromHistory(history[historyIndex]);
  }
});

errorDismiss.addEventListener("click", () => {
  hideOverlays();
  if (historyIndex > 0) {
    historyIndex--;
    const url = history[historyIndex];
    addressBar.value = url;
    updateNavButtons();
  } else {
    showWelcome();
  }
});

function navigateFromHistory(url) {
  const slashIdx = url.indexOf("/");
  const host = slashIdx >= 0 ? url.substring(0, slashIdx) : url;
  const path = slashIdx >= 0 ? url.substring(slashIdx) : "/";
  doNavigate(host, path);
}

function showWelcome() {
  hideOverlays();
  pageFrame.style.display = "none";
  pageFrame.src = "about:blank";
  welcome.style.display = "flex";
  addressBar.value = "";
  setStatus("Ready");
}

// ── Error categorization ──

function categorizeError(msg) {
  const lower = msg.toLowerCase();
  if (lower.includes("name index is not connected")) {
    return {
      title: "Name Not Available Yet",
      detail: "Bitcoin name resolution requires a synced name index. Use an npub for now.",
    };
  }
  if (lower.includes("invalid nsite address")) {
    return {
      title: "Invalid Address",
      detail: msg,
    };
  }
  if (lower.includes("invalid npub")) {
    return {
      title: "Invalid npub",
      detail: "The npub address could not be decoded. Check that it's a valid Nostr public key.",
    };
  }
  if (lower.includes("manifest not found")) {
    return {
      title: "Site Not Found",
      detail: "No nsite manifest was found for this pubkey. The site may not exist or relays may be unreachable.",
    };
  }
  if (lower.includes("path not found")) {
    return {
      title: "Page Not Found",
      detail: msg.replace("path not found in manifest: ", "The path does not exist in this site's manifest: "),
    };
  }
  if (lower.includes("no blossom servers") || lower.includes("http error")) {
    return {
      title: "Content Unavailable",
      detail: "Could not fetch the site content from Blossom servers. They may be temporarily down.",
    };
  }
  if (lower.includes("hash mismatch")) {
    return {
      title: "Integrity Error",
      detail: "The downloaded content did not match its expected SHA256 hash. The Blossom server may be serving corrupted data.",
    };
  }
  if (lower.includes("relay")) {
    return {
      title: "Relay Error",
      detail: "Could not connect to Nostr relays. Check your internet connection.",
    };
  }
  return {
    title: "Navigation Failed",
    detail: msg,
  };
}

// Hide loading when iframe finishes loading
pageFrame.addEventListener("load", () => {
  hideOverlays();
});

// Intercept nsite:// link clicks from within rendered pages
window.addEventListener("message", (e) => {
  if (e.data && e.data.type === "nsite-navigate" && e.data.url) {
    navigate(e.data.url);
  }
});

// Keyboard shortcut: Cmd/Ctrl+L to focus the address bar
document.addEventListener("keydown", (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === "l") {
    e.preventDefault();
    addressBar.focus();
    addressBar.select();
  }
});

// Load nsite://titan as the default homepage
navigate("titan");
