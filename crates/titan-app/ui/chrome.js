// Titan browser chrome — transparent overlay with toolbar, bookmarks dropdown
// Communicates with Rust backend via Tauri commands and events

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const addressBar = document.getElementById("address-bar");
const btnBack = document.getElementById("btn-back");
const btnForward = document.getElementById("btn-forward");
const btnRefresh = document.getElementById("btn-refresh");
const btnStar = document.getElementById("btn-star");
const btnBookmarks = document.getElementById("btn-bookmarks");
const bookmarksDropdown = document.getElementById("bookmarks-dropdown");
const bookmarksList = document.getElementById("bookmarks-list");
const bookmarksEmpty = document.getElementById("bookmarks-empty");
const loadingBar = document.getElementById("loading-bar");

let currentUrl = "";

// ── Navigation ──

async function navigate(input) {
  const cleaned = input.trim().replace(/^nsite:\/\//, "");
  if (!cleaned) return;

  setStatus(`Resolving ${cleaned}...`);
  showLoading();

  try {
    const displayUrl = await invoke("navigate", { url: cleaned });
    addressBar.value = displayUrl;
    currentUrl = displayUrl;
    setStatus(`Loaded ${displayUrl}`);
    btnBack.disabled = false;
    updateStarState();
    hideLoading();
  } catch (err) {
    const msg = typeof err === "string" ? err : err.message || JSON.stringify(err);
    setStatus(`Error: ${msg}`);
    hideLoading();
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

async function toggleBookmarksDropdown() {
  if (bookmarksDropdown.style.display === "none") {
    await renderBookmarks();
    bookmarksDropdown.style.display = "block";
  } else {
    bookmarksDropdown.style.display = "none";
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
        <div class="bookmark-title">${escapeHtml(b.title)}</div>
        <div class="bookmark-url">nsite://${escapeHtml(b.url)}</div>
      </div>
      <button class="bookmark-delete" title="Remove">&#x2715;</button>
    `;

    item.addEventListener("click", () => {
      bookmarksDropdown.style.display = "none";
      navigate(url);
    });

    item.querySelector(".bookmark-delete").addEventListener("click", async (e) => {
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
btnBookmarks.addEventListener("click", toggleBookmarksDropdown);

// Close dropdown when clicking outside
document.addEventListener("click", (e) => {
  if (bookmarksDropdown.style.display !== "none" &&
      !bookmarksDropdown.contains(e.target) &&
      e.target !== btnBookmarks) {
    bookmarksDropdown.style.display = "none";
  }
});

// Listen for URL updates from content webview
listen("page-loaded", (event) => {
  const url = event.payload;
  if (url) {
    addressBar.value = url;
    currentUrl = url;
    setStatus(`Loaded ${url}`);
    updateStarState();
    hideLoading();
  }
});

listen("nsite-link-clicked", (event) => {
  if (event.payload) navigate(event.payload);
});

listen("status", (event) => {
  if (event.payload) setStatus(event.payload);
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
});

function setStatus(text) {
  // Status shown via loading bar animation; text available for debugging
}

function showLoading() {
  loadingBar.className = "loading";
}

function hideLoading() {
  loadingBar.className = "done";
  setTimeout(() => { loadingBar.className = ""; }, 600);
}

function escapeHtml(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// Navigate to nsite://titan on load
navigate("titan");
