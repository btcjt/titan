// Titan browser chrome — address bar + navigation controls
// Communicates with Rust backend via Tauri commands and events

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const addressBar = document.getElementById("address-bar");
const btnBack = document.getElementById("btn-back");
const btnForward = document.getElementById("btn-forward");
const btnRefresh = document.getElementById("btn-refresh");
const statusText = document.getElementById("status-text");

// ── Navigation ──

async function navigate(input) {
  const cleaned = input.trim().replace(/^nsite:\/\//, "");
  if (!cleaned) return;

  setStatus(`Resolving ${cleaned}...`);

  try {
    const displayUrl = await invoke("navigate", { url: cleaned });
    addressBar.value = displayUrl;
    setStatus(`Loaded ${displayUrl}`);
    btnBack.disabled = false;
  } catch (err) {
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
    setStatus(`Error: ${msg}`);
  }
}

// ── Event listeners ──

addressBar.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    navigate(addressBar.value);
  }
});

btnBack.addEventListener("click", () => invoke("go_back"));
btnForward.addEventListener("click", () => invoke("go_forward"));
btnRefresh.addEventListener("click", () => invoke("refresh"));

// Listen for URL updates when the content webview navigates
listen("page-loaded", (event) => {
  const url = event.payload;
  if (url) {
    addressBar.value = url;
    setStatus(`Loaded ${url}`);
  }
});

// Listen for status updates from Rust
listen("status", (event) => {
  if (event.payload) {
    setStatus(event.payload);
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

function setStatus(text) {
  statusText.textContent = text;
}

// Handle nsite:// link clicks intercepted by the content webview
listen("nsite-link-clicked", (event) => {
  if (event.payload) {
    navigate(event.payload);
  }
});

// Navigate to nsite://titan on load
navigate("titan");
