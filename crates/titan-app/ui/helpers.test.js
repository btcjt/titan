// Unit tests for the security-critical helpers in helpers.js.
//
// Run with: `node crates/titan-app/ui/helpers.test.js`
// No dependencies beyond Node's stdlib `assert` and `url` modules.
//
// These helpers gate untrusted data from Nostr (kind-0 profile fields,
// indexer name records) before it gets interpolated into the chrome
// webview's HTML. A regression here would reopen the XSS path identified
// in the security audit — specifically the `profile.website` field
// becoming `<a href="javascript:...">` in the site info panel.

const assert = require("assert");
const {
  escapeHtml,
  escapeAttr,
  isSafeHttpUrl,
  isHex,
} = require("./helpers.js");

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed += 1;
    console.log(`  ok   ${name}`);
  } catch (e) {
    failed += 1;
    console.error(`  FAIL ${name}`);
    console.error(`       ${e.message}`);
  }
}

console.log("escapeHtml");

test("escapes ampersand, angle brackets, and both quote types", () => {
  assert.strictEqual(
    escapeHtml(`<script>alert("hi" + 'bye')</script>&end`),
    "&lt;script&gt;alert(&quot;hi&quot; + &#39;bye&#39;)&lt;/script&gt;&amp;end",
  );
});

test("ampersand is escaped first so &lt; doesn't become &amp;lt;", () => {
  // If escaping order were wrong, "<" would become "&amp;lt;". Regression
  // guard — the original implementation had this right but a careless
  // refactor could break it.
  assert.strictEqual(escapeHtml("<"), "&lt;");
  assert.strictEqual(escapeHtml("&"), "&amp;");
  assert.strictEqual(escapeHtml("&<"), "&amp;&lt;");
});

test("coerces non-strings instead of throwing", () => {
  assert.strictEqual(escapeHtml(42), "42");
  assert.strictEqual(escapeHtml(null), "null");
  assert.strictEqual(escapeHtml(undefined), "undefined");
});

test("plain text passes through unchanged", () => {
  assert.strictEqual(escapeHtml("hello world 123"), "hello world 123");
});

test("unicode is preserved", () => {
  assert.strictEqual(escapeHtml("Hëllo 🌙"), "Hëllo 🌙");
});

console.log("escapeAttr");

test("escapes both quote types so attribute context is safe", () => {
  // Attribute context uses double quotes. escapeAttr must neutralize both
  // to prevent breaking out of either kind of quoted attribute.
  const out = escapeAttr(`" onclick="alert(1)`);
  assert.ok(!out.includes(`"`), `output should not contain raw quote: ${out}`);
  assert.ok(out.includes("&quot;"));
});

test("escapes single quote", () => {
  assert.ok(escapeAttr("'").includes("&#39;"));
});

test("coerces non-strings", () => {
  assert.strictEqual(escapeAttr(42), "42");
});

console.log("isSafeHttpUrl");

test("accepts http and https", () => {
  assert.strictEqual(isSafeHttpUrl("http://example.com"), true);
  assert.strictEqual(isSafeHttpUrl("https://example.com"), true);
  assert.strictEqual(isSafeHttpUrl("https://sub.example.com/path?q=1"), true);
});

test("rejects javascript: scheme (the XSS vector)", () => {
  // This is the whole point of the helper. The original bug:
  // <a href="${escapeAttr(profile.website)}"> where a hostile kind-0
  // profile set website to "javascript:fetch('/steal')" would execute
  // in the chrome context on click because CSP has 'unsafe-inline'.
  assert.strictEqual(
    isSafeHttpUrl("javascript:alert(document.cookie)"),
    false,
  );
  assert.strictEqual(
    isSafeHttpUrl("JavaScript:alert(1)"),
    false,
    "scheme match must be case-insensitive",
  );
  assert.strictEqual(isSafeHttpUrl("  javascript:alert(1)"), false);
});

test("rejects data: URLs", () => {
  // data:text/html,<script> is another XSS vector — must block.
  assert.strictEqual(
    isSafeHttpUrl("data:text/html,<script>alert(1)</script>"),
    false,
  );
});

test("rejects file:, ftp:, vbscript:, blob:, about:", () => {
  for (const u of [
    "file:///etc/passwd",
    "ftp://example.com/",
    "vbscript:msgbox(1)",
    "blob:https://example.com/uuid",
    "about:blank",
  ]) {
    assert.strictEqual(isSafeHttpUrl(u), false, `should reject: ${u}`);
  }
});

test("rejects non-strings and empty strings", () => {
  assert.strictEqual(isSafeHttpUrl(null), false);
  assert.strictEqual(isSafeHttpUrl(undefined), false);
  assert.strictEqual(isSafeHttpUrl(""), false);
  assert.strictEqual(isSafeHttpUrl(42), false);
  assert.strictEqual(isSafeHttpUrl({}), false);
});

test("rejects malformed URLs that would throw", () => {
  assert.strictEqual(isSafeHttpUrl("not a url"), false);
  assert.strictEqual(isSafeHttpUrl("://missing-scheme"), false);
});

test("rejects URLs over 2048 chars (DoS defense)", () => {
  const long = "https://" + "a".repeat(3000) + ".com";
  assert.strictEqual(isSafeHttpUrl(long), false);
});

console.log("isHex");

test("accepts lowercase and uppercase hex", () => {
  assert.strictEqual(isHex("deadbeef"), true);
  assert.strictEqual(isHex("DEADBEEF"), true);
  assert.strictEqual(isHex("DeAdBeEf"), true);
});

test("accepts txid with exact length check", () => {
  const txid = "a".repeat(64);
  assert.strictEqual(isHex(txid, 64), true);
});

test("rejects wrong length when expectedLen is set", () => {
  assert.strictEqual(isHex("deadbeef", 64), false);
  assert.strictEqual(isHex("a".repeat(63), 64), false);
  assert.strictEqual(isHex("a".repeat(65), 64), false);
});

test("rejects non-hex characters", () => {
  // The critical case: an attacker-controlled indexer event shoving
  // '"><script>alert(1)</script>' into a txid field. This must be
  // rejected before being interpolated into an <a href=...>.
  assert.strictEqual(
    isHex(`abcd"><script>alert(1)</script>`, 64),
    false,
  );
  assert.strictEqual(isHex("xyz"), false);
  assert.strictEqual(isHex("zz"), false);
});

test("rejects empty string and non-strings", () => {
  assert.strictEqual(isHex(""), false);
  assert.strictEqual(isHex(null), false);
  assert.strictEqual(isHex(undefined), false);
  assert.strictEqual(isHex(123), false);
});

console.log("");
console.log(`${passed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
