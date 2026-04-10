// Security-critical pure helpers used by chrome.js for sanitizing
// untrusted data (kind-0 profile fields, indexer txids, etc.) before
// interpolation into HTML or URLs.
//
// Kept in a separate file so the Node test runner can load and unit test
// them without pulling in chrome.js's top-level DOM setup. This file is
// loaded before chrome.js in chrome.html and also required by
// chrome.test.js.

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function escapeAttr(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;")
    .replace(/</g, "&lt;");
}

// Returns true only for http:// or https:// URLs. Used to gate rendering
// of untrusted URLs (profile.website, profile.picture) as anchor hrefs or
// image srcs. Without this check, a kind-0 profile with
// `website: "javascript:..."` would XSS the chrome webview on click —
// the CSP has 'unsafe-inline' which permits javascript: URLs.
function isSafeHttpUrl(s) {
  if (typeof s !== "string" || s.length === 0 || s.length > 2048) return false;
  try {
    const u = new URL(s);
    return u.protocol === "http:" || u.protocol === "https:";
  } catch {
    return false;
  }
}

// Validates a hex string (e.g. a Bitcoin txid) before it gets interpolated
// into a URL path. Defensive — the indexer should only ever publish valid
// hex, but we don't want a malformed indexer event to break out of an href.
function isHex(s, expectedLen) {
  if (typeof s !== "string") return false;
  if (expectedLen !== undefined && s.length !== expectedLen) return false;
  return /^[0-9a-fA-F]+$/.test(s);
}

// Export for Node-based unit tests. In the browser `module` is undefined
// and this is a no-op, so chrome.js (loaded after this file) can still
// reference these helpers as globals.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { escapeHtml, escapeAttr, isSafeHttpUrl, isHex };
}
