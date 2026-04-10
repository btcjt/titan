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

// Build a bash-friendly `curl` command from a captured NetworkEvent.
// Used by the devtools Network tab's "Copy as cURL" button.
//
// Handles method, URL, headers, and an optional request body. Quotes
// shell-unsafe values using single-quote wrapping with the standard
// `'\''` escape for embedded single quotes. Preserves the common
// `-H` / `--data-raw` conventions that Chrome and Firefox devtools use
// so users can paste the result into any curl-compatible tool.
function buildCurlCommand(event) {
  if (!event || !event.url) return "";
  const parts = ["curl"];
  const method = (event.method || "GET").toUpperCase();

  if (method !== "GET") {
    parts.push("-X", method);
  }

  parts.push(shellQuote(event.url));

  const headers = Array.isArray(event.request_headers)
    ? event.request_headers
    : [];
  for (const [name, value] of headers) {
    if (typeof name !== "string") continue;
    // Skip the content-length header — curl computes it itself.
    if (name.toLowerCase() === "content-length") continue;
    parts.push("-H", shellQuote(`${name}: ${value == null ? "" : value}`));
  }

  if (typeof event.request_body === "string" && event.request_body.length > 0) {
    // --data-raw preserves the body exactly without curl's at-file
    // special-case on @-prefixed strings.
    parts.push("--data-raw", shellQuote(event.request_body));
  }

  return parts.join(" ");
}

// Quote a string for bash, using single quotes and escaping any
// embedded single quotes via the '\'' trick.
function shellQuote(s) {
  if (s == null) return "''";
  const str = String(s);
  if (str === "") return "''";
  // Already-safe: only printable ASCII that bash treats as a word
  if (/^[a-zA-Z0-9_\-./:@%+=,]+$/.test(str)) return str;
  return "'" + str.replace(/'/g, "'\\''") + "'";
}

// Export for Node-based unit tests. In the browser `module` is undefined
// and this is a no-op, so chrome.js (loaded after this file) can still
// reference these helpers as globals.
if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    escapeHtml,
    escapeAttr,
    isSafeHttpUrl,
    isHex,
    buildCurlCommand,
    shellQuote,
  };
}
