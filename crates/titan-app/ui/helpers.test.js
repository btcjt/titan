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
  buildCurlCommand,
  shellQuote,
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

console.log("shellQuote");

test("passes safe characters through unquoted", () => {
  assert.strictEqual(shellQuote("abc"), "abc");
  assert.strictEqual(shellQuote("example.com"), "example.com");
  assert.strictEqual(shellQuote("/path/to/thing"), "/path/to/thing");
  assert.strictEqual(shellQuote("a-b_c.d"), "a-b_c.d");
});

test("wraps unsafe strings in single quotes", () => {
  assert.strictEqual(shellQuote("hello world"), "'hello world'");
  assert.strictEqual(shellQuote("with $var"), "'with $var'");
  assert.strictEqual(shellQuote("with `backticks`"), "'with `backticks`'");
});

test("escapes embedded single quotes via '\\''", () => {
  // Classic bash single-quote escape: close, escape, reopen
  assert.strictEqual(shellQuote("it's"), "'it'\\''s'");
  assert.strictEqual(shellQuote("'start"), "''\\''start'");
  assert.strictEqual(shellQuote("end'"), "'end'\\'''");
});

test("quotes empty string as ''", () => {
  assert.strictEqual(shellQuote(""), "''");
});

test("coerces null/undefined safely", () => {
  assert.strictEqual(shellQuote(null), "''");
  assert.strictEqual(shellQuote(undefined), "''");
  assert.strictEqual(shellQuote(42), "42");
});

console.log("buildCurlCommand");

test("simple GET with URL only", () => {
  const cmd = buildCurlCommand({
    method: "GET",
    url: "https://example.com/api",
  });
  assert.strictEqual(cmd, "curl https://example.com/api");
});

test("GET is implicit — no -X GET", () => {
  // Chrome devtools and the common copy-as-cURL convention both
  // omit -X for GET since it's the curl default.
  const cmd = buildCurlCommand({ method: "GET", url: "https://x/" });
  assert.ok(!cmd.includes("-X GET"));
});

test("POST adds -X POST", () => {
  const cmd = buildCurlCommand({ method: "POST", url: "https://x/" });
  assert.ok(cmd.includes("-X POST"));
});

test("headers render as -H 'name: value'", () => {
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://example.com",
    request_headers: [
      ["content-type", "application/json"],
      ["x-auth", "secret"],
    ],
  });
  assert.ok(cmd.includes("-H 'content-type: application/json'"));
  assert.ok(cmd.includes("-H 'x-auth: secret'"));
});

test("content-length header is stripped (curl computes it)", () => {
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://x/",
    request_headers: [["content-length", "42"]],
  });
  assert.ok(!cmd.toLowerCase().includes("content-length"));
});

test("request body renders as --data-raw with proper quoting", () => {
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://x/",
    request_body: '{"hello":"world"}',
  });
  assert.ok(cmd.includes(`--data-raw '{"hello":"world"}'`));
});

test("request body with embedded single quotes is escaped correctly", () => {
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://x/",
    request_body: "it's a trap",
  });
  assert.ok(cmd.includes(`--data-raw 'it'\\''s a trap'`));
});

test("missing event or url returns empty string", () => {
  assert.strictEqual(buildCurlCommand(null), "");
  assert.strictEqual(buildCurlCommand({}), "");
  assert.strictEqual(buildCurlCommand({ method: "GET" }), "");
});

test("handles missing headers array gracefully", () => {
  const cmd = buildCurlCommand({
    method: "GET",
    url: "https://x/",
    // request_headers intentionally omitted
  });
  assert.strictEqual(cmd, "curl https://x/");
});

test("handles string request_body only (not FormData etc.)", () => {
  // Spec: only strings get --data-raw. Objects/buffers from our
  // wrapper would have been stringified or dropped upstream.
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://x/",
    request_body: "plain=form&data=1",
  });
  assert.ok(cmd.includes("--data-raw 'plain=form&data=1'"));
});

test("full real-world example round-trips sensibly", () => {
  // Capture from a hypothetical POST /api/events with JSON body.
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://relay.example.com/api/events",
    request_headers: [
      ["content-type", "application/json"],
      ["authorization", "Bearer abc123"],
    ],
    request_body: '{"kind":1,"content":"hi"}',
  });
  // The components should all be present and shellable:
  assert.ok(cmd.startsWith("curl -X POST"));
  assert.ok(cmd.includes("https://relay.example.com/api/events"));
  assert.ok(cmd.includes("-H 'content-type: application/json'"));
  assert.ok(cmd.includes("-H 'authorization: Bearer abc123'"));
  assert.ok(cmd.includes(`--data-raw '{"kind":1,"content":"hi"}'`));
});

console.log("buildCurlCommand (adversarial)");

// These tests try to break the curl builder or escape a shell context.
// The goal is to prove the output is safe to paste into a terminal,
// even when the input came from a malicious page. If any of these
// fail, treat it as a shell-injection bug in the builder, not a
// reason to loosen the test.

test("command substitution via $() is neutralized inside single quotes", () => {
  // $(...) in a bash double-quoted or unquoted context runs a
  // subshell. Inside single quotes it's literal, which is why
  // shellQuote always wraps unsafe input. Verify the command
  // substitution is not preserved verbatim.
  const cmd = buildCurlCommand({
    method: "GET",
    url: "https://example.com/$(rm -rf /)",
  });
  // The $(...) should be inside single quotes, making it literal
  assert.ok(cmd.includes("'https://example.com/$(rm -rf /)'"));
});

test("backtick command substitution is neutralized", () => {
  const cmd = buildCurlCommand({
    method: "GET",
    url: "https://example.com/`whoami`",
  });
  assert.ok(cmd.includes("'https://example.com/`whoami`'"));
});

test("semicolon cannot break out of the command", () => {
  const cmd = buildCurlCommand({
    method: "GET",
    url: "https://x/; rm -rf ~",
  });
  // Single quoted — the semicolon is literal, not a command separator
  assert.ok(cmd.includes("'https://x/; rm -rf ~'"));
  // And there's no bare "; rm -rf" outside of quotes
  // The full command should start with curl, then the quoted URL.
  assert.strictEqual(cmd.split("'").length, 3); // open quote, content, close quote
});

test("newline in a header value cannot inject a second curl arg", () => {
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://x/",
    request_headers: [["x-evil", "first\nX-Injected: pwned"]],
  });
  // The entire header must be inside a single quoted string
  assert.ok(cmd.includes("'x-evil: first\nX-Injected: pwned'"));
  // There must not be a second unquoted -H
  const matches = cmd.match(/-H /g) || [];
  assert.strictEqual(matches.length, 1, "should only have one -H flag");
});

test("header name with embedded single quote doesn't corrupt the command", () => {
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://x/",
    request_headers: [["x'evil", "value"]],
  });
  // Verify the quoted section is well-formed (the '\'' escape triggers)
  assert.ok(cmd.includes(`-H 'x'\\''evil: value'`));
});

test("request body with embedded null byte survives (shell takes it literally)", () => {
  // Bash single quotes preserve any character except the single
  // quote itself, including NUL. The output should contain the
  // literal NUL byte.
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://x/",
    request_body: "a\0b",
  });
  assert.ok(cmd.includes("a\0b"));
});

test("extremely long request body does not crash the builder", () => {
  const body = "x".repeat(100_000);
  const cmd = buildCurlCommand({
    method: "POST",
    url: "https://x/",
    request_body: body,
  });
  assert.ok(cmd.length > 100_000);
  assert.ok(cmd.includes("--data-raw"));
});

test("method is uppercased consistently", () => {
  // Accept lowercase input (matches what some content pages might
  // pass in via fetch init), but produce canonical uppercase in the
  // emitted command.
  const cmd = buildCurlCommand({
    method: "post",
    url: "https://x/",
  });
  assert.ok(cmd.includes("-X POST"));
});

test("headers with undefined/null values do not crash", () => {
  // Defensive: a bad page might have headers with null values.
  // buildCurlCommand should render them as empty strings rather
  // than throwing or producing "undefined"/"null" literal strings.
  const cmd = buildCurlCommand({
    method: "GET",
    url: "https://x/",
    request_headers: [
      ["x-null", null],
      ["x-undef", undefined],
    ],
  });
  assert.ok(cmd.includes("-H 'x-null: '"));
  assert.ok(cmd.includes("-H 'x-undef: '"));
  // And definitely not the literal strings
  assert.ok(!cmd.includes("x-null: null"));
  assert.ok(!cmd.includes("x-undef: undefined"));
});

test("non-array request_headers does not crash", () => {
  // If the captured event somehow has request_headers as a string
  // or object instead of an array, the builder should ignore it
  // rather than throwing.
  const cmd = buildCurlCommand({
    method: "GET",
    url: "https://x/",
    request_headers: "not an array",
  });
  assert.strictEqual(cmd, "curl https://x/");
});

test("url with special chars gets single-quote wrapped", () => {
  const cases = [
    "https://x/?a=1&b=2",
    "https://x/#fragment",
    "https://x/path with space",
    "https://x/[bracket]",
    "https://x/{brace}",
    "https://x/pipe|command",
  ];
  for (const url of cases) {
    const cmd = buildCurlCommand({ method: "GET", url });
    assert.ok(
      cmd.includes(`'${url}'`),
      `expected '${url}' to be wrapped: got ${cmd}`,
    );
  }
});

console.log("escapeHtml (adversarial)");

test("does not mangle already-escaped entities", () => {
  // Input that already contains &amp; should round-trip to &amp;amp;
  // — which is correct behavior for a context that will be
  // re-interpreted as HTML. Re-escaping is the safe choice.
  assert.strictEqual(escapeHtml("&amp;"), "&amp;amp;");
});

test("ampersand-first ordering prevents double-escape corruption", () => {
  // If the implementation escaped & after <, the resulting &lt;
  // would become &amp;lt;. Regression guard — this has bitten us
  // in other projects.
  assert.strictEqual(escapeHtml("&<&>&\"&'"), "&amp;&lt;&amp;&gt;&amp;&quot;&amp;&#39;");
});

test("surrogate pair emoji passes through intact", () => {
  // 🔒 is U+1F512, a surrogate pair in UTF-16. Regex replace
  // operations that work per-codepoint can accidentally break this.
  assert.strictEqual(escapeHtml("🔒 locked"), "🔒 locked");
});

test("HTML entity in attribute context stays safe", () => {
  // An attacker trying to break out of an attribute context would
  // use " or ' or >. All should be neutralized.
  const input = `" onclick="alert(1)`;
  const escaped = escapeAttr(input);
  assert.ok(!escaped.includes('"'));
  assert.ok(escaped.includes("&quot;"));
});

console.log("isSafeHttpUrl (adversarial)");

test("leading whitespace javascript: is rejected", () => {
  // WHATWG URL parser trims leading whitespace, so " javascript:..."
  // becomes "javascript:..." after parsing. This should still be
  // rejected.
  assert.strictEqual(isSafeHttpUrl("   javascript:alert(1)"), false);
  assert.strictEqual(isSafeHttpUrl("\tjavascript:alert(1)"), false);
  assert.strictEqual(isSafeHttpUrl("\njavascript:alert(1)"), false);
});

test("mixed-case javascript: scheme is rejected", () => {
  // URL parser lowercases the scheme, so any case variation should
  // be caught.
  const variants = [
    "JAVASCRIPT:alert(1)",
    "JavaScript:alert(1)",
    "jAvAsCrIpT:alert(1)",
  ];
  for (const v of variants) {
    assert.strictEqual(isSafeHttpUrl(v), false, `expected ${v} to be rejected`);
  }
});

test("URL with embedded javascript: in path is allowed (not a scheme)", () => {
  // A URL whose PATH contains "javascript:" is fine — only the
  // scheme matters. This is a real pattern (escape.fee/link/javascript:foo)
  // and blocking it would be overkill.
  assert.strictEqual(
    isSafeHttpUrl("https://example.com/path/javascript:alert(1)"),
    true,
  );
});

test("URL over 2048 chars is rejected even if scheme is https", () => {
  const long = "https://" + "a".repeat(3000) + ".com";
  assert.strictEqual(isSafeHttpUrl(long), false);
});

test("URL at exactly 2048 chars is accepted", () => {
  // 2048 is the inclusive upper bound. We want to verify the
  // boundary behavior so a refactor to <= / < doesn't silently
  // break either case.
  const prefix = "https://example.com/";
  const fill = "a".repeat(2048 - prefix.length);
  const url = prefix + fill;
  assert.strictEqual(url.length, 2048);
  assert.strictEqual(isSafeHttpUrl(url), true);
});

test("URL at 2049 chars is rejected", () => {
  const url = "https://example.com/" + "a".repeat(2049 - "https://example.com/".length);
  assert.strictEqual(url.length, 2049);
  assert.strictEqual(isSafeHttpUrl(url), false);
});

test("userinfo-in-url is accepted if scheme is https", () => {
  // https://user:pass@example.com/ is a legitimate URL (deprecated
  // but not invalid). isSafeHttpUrl only cares about the scheme.
  assert.strictEqual(
    isSafeHttpUrl("https://user:pass@example.com/"),
    true,
  );
});

console.log("isHex (adversarial)");

test("all-whitespace string is rejected", () => {
  assert.strictEqual(isHex("   "), false);
  assert.strictEqual(isHex("\t\n\r"), false);
});

test("hex with surrounding whitespace is rejected (no auto-trim)", () => {
  // isHex does not trim — callers who want to accept padded hex
  // should trim first. Regression guard.
  assert.strictEqual(isHex(" abc ", 5), false);
  assert.strictEqual(isHex("abc ", 4), false);
});

test("expectedLen of 0 rejects any string", () => {
  // Strict equality on length. An empty string passes regex-wise
  // but the empty-string rule at the top of isHex rejects it.
  // expectedLen=0 asking for a non-existent empty hex is also
  // rejected (a sane default).
  assert.strictEqual(isHex("", 0), false);
});

test("hex with internal spaces is rejected", () => {
  assert.strictEqual(isHex("ab cd"), false);
  assert.strictEqual(isHex("ab\tcd"), false);
});

test("hex with unicode lookalike characters is rejected", () => {
  // Fullwidth digits from East Asian character blocks look like
  // ASCII digits but are different code points. Our regex uses
  // [0-9a-fA-F] which is strict ASCII, so these fail.
  assert.strictEqual(isHex("１２３４"), false); // fullwidth digits
  assert.strictEqual(isHex("ａｂｃｄ"), false); // fullwidth letters
});

test("64-char hex txid passes the common bitcoin validation", () => {
  // Realistic Bitcoin txid example — 64 lowercase hex chars.
  assert.strictEqual(
    isHex(
      "322ab8800aa8d926161ff398d5d0b6c851c66679830fe05b223a548794e7002f",
      64,
    ),
    true,
  );
});

console.log("");
console.log(`${passed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
