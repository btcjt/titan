// Titan browser chrome — toolbar + side panels
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
const panelClose = document.getElementById("panel-close");
const bookmarksList = document.getElementById("bookmarks-list");
const bookmarksEmpty = document.getElementById("bookmarks-empty");

let currentUrl = "";
const TOOLBAR_HEIGHT = 78;
const PANEL_WIDTH = 280;

// ── Content Webview Layout ──

async function updateContentLayout() {
  const rightOffset = sidePanel.style.display !== "none" ? PANEL_WIDTH : 0;
  await invoke("resize_content", { top: TOOLBAR_HEIGHT, right: rightOffset });
}

// ── Navigation ──

async function navigate(input) {
  const cleaned = input.trim().replace(/^nsite:\/\//, "");
  if (!cleaned) return;

  showLoading();

  try {
    const displayUrl = await invoke("navigate", { url: cleaned });
    addressBar.value = displayUrl;
    currentUrl = displayUrl;
    btnBack.disabled = false;
    updateStarState();
    hideLoading();
  } catch (err) {
    hideLoading();
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
    // Navigate to internal error page
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
  } else {
    const title = currentUrl.split("/")[0] || currentUrl;
    await invoke("add_bookmark", { url: currentUrl, title });
  }
  updateStarState();
  // Refresh panel if open
  if (sidePanel.style.display !== "none") {
    await renderBookmarks();
  }
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

// ── Side Panel ──

async function togglePanel() {
  if (sidePanel.style.display !== "none") {
    closePanel();
  } else {
    await openBookmarksPanel();
  }
}

async function openBookmarksPanel() {
  await renderBookmarks();
  sidePanel.style.display = "flex";
  await updateContentLayout();
}

async function closePanel() {
  sidePanel.style.display = "none";
  await updateContentLayout();
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
        <div class="bookmark-title">${escapeHtml(b.title)}</div>
        <div class="bookmark-url">nsite://${escapeHtml(b.url)}</div>
      </div>
      <button class="bookmark-delete" title="Remove">&#x2715;</button>
    `;

    item.addEventListener("click", () => {
      navigate(url);
    });

    item
      .querySelector(".bookmark-delete")
      .addEventListener("click", async (e) => {
        e.stopPropagation();
        await invoke("remove_bookmark", { url });
        await renderBookmarks();
        updateStarState();
      });

    bookmarksList.appendChild(item);
  }
}

// ── Event Listeners ──

addressBar.addEventListener("keydown", (e) => {
  if (e.key === "Enter") navigate(addressBar.value);
});

btnBack.addEventListener("click", () => invoke("go_back"));
btnForward.addEventListener("click", () => invoke("go_forward"));
btnRefresh.addEventListener("click", () => invoke("refresh"));
btnStar.addEventListener("click", toggleBookmark);
btnBookmarks.addEventListener("click", togglePanel);
panelClose.addEventListener("click", closePanel);

listen("page-loaded", (event) => {
  if (event.payload) {
    addressBar.value = event.payload;
    currentUrl = event.payload;
    updateStarState();
    hideLoading();
  }
});

listen("nsite-link-clicked", (event) => {
  if (event.payload) navigate(event.payload);
});

// Keyboard shortcuts
document.addEventListener("keydown", (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === "l") {
    e.preventDefault();
    addressBar.focus();
    addressBar.select();
  }
  if ((e.metaKey || e.ctrlKey) && e.key === "d") {
    e.preventDefault();
    toggleBookmark();
  }
  if (e.key === "Escape" && sidePanel.style.display !== "none") {
    closePanel();
  }
});

function showLoading() {
  loadingBar.className = "loading";
}

function hideLoading() {
  loadingBar.className = "done";
  setTimeout(() => {
    loadingBar.className = "";
  }, 600);
}

function escapeHtml(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// Keep content webview sized correctly on window resize
window.addEventListener("resize", () => updateContentLayout());

// Set initial content layout, then navigate to homepage
updateContentLayout().then(() => navigate("titan"));
