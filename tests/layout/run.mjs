// Layout tests: renders the REAL production build in headless Chrome, with the Tauri IPC mocked
// (mock.js) and deliberately awkward data, and fails if any page or modal overflows, scrolls
// horizontally, clips a control, or wraps short text. This is what catches "a long repo name
// pushed the toolbar / close button / page wider" regressions that unit tests can't see.
//
//   npm run test:layout
//
// Zero npm dependencies: it reuses a Chrome/Chromium already on the machine (CHROME_BIN to
// override). With no browser found it skips (exit 0) unless LAYOUT_REQUIRE_BROWSER is set. It is not
// part of the CI workflow (deliberately — see docs/frontend.md), so CI alone never makes it fail.
// Optional narrowing for debugging: LAYOUT_ROUTES="/logs,/settings" LAYOUT_WIDTHS="1024,1280".
// LAYOUT_PROBE="main th" prints the rendered width of every match (to see how columns share space).
// See docs/frontend.md ("Layout tests") for what is and isn't covered.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../..");

// Wraps that are fine: a short span inside a running description sentence flows across lines by
// design (the detector flags any short leaf element whose text spans two lines).
const ALLOWED_WRAPS = [/Ignore case/];

const PAGE_WIDTHS = "1024,1100,1151,1152,1200,1279,1280,1366,1536,1600";
// A page that rendered almost nothing is a broken mock/route, not a pass.
const MIN_PAGE_TEXT = 300;

function findChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  ].filter(Boolean);
  for (const c of candidates) if (fs.existsSync(c)) return c;
  for (const name of ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"]) {
    const r = spawnSync(process.platform === "win32" ? "where" : "which", [name], { encoding: "utf8" });
    if (r.status === 0 && r.stdout.trim()) return r.stdout.trim().split(/\r?\n/)[0];
  }
  return null;
}

const chrome = findChrome();
if (!chrome) {
  const required = process.env.LAYOUT_REQUIRE_BROWSER;
  console.log(`layout: no Chrome/Chromium found (set CHROME_BIN) — ${required ? "FAILING (LAYOUT_REQUIRE_BROWSER)" : "skipping"}`);
  process.exit(required ? 1 : 0);
}

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "resty-layout-"));
const dist = path.join(tmp, "dist");

console.log("layout: building the app…");
const build = spawnSync(process.execPath, [path.join(root, "node_modules/vite/bin/vite.js"), "build", "--outDir", dist, "--emptyOutDir", "--logLevel", "error"], { cwd: root, encoding: "utf8" });
if (build.status !== 0) {
  console.error(build.stdout, build.stderr);
  fs.rmSync(tmp, { recursive: true, force: true });
  process.exit(1);
}
const indexHtml = fs.readFileSync(path.join(dist, "index.html"), "utf8").replace("<head>", '<head><script src="/mock.js"></script>');

const MIME = { ".js": "text/javascript", ".css": "text/css", ".html": "text/html", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json", ".woff2": "font/woff2" };
const server = http.createServer((req, res) => {
  const p = decodeURIComponent(req.url.split("?")[0]);
  const send = (body, type) => { res.writeHead(200, { "Content-Type": type }); res.end(body); };
  if (["/mock.js", "/pages.html", "/modals.html"].includes(p)) return send(fs.readFileSync(path.join(here, p)), MIME[path.extname(p)]);
  const fp = path.join(dist, p);
  if (p !== "/" && fp.startsWith(dist) && fs.existsSync(fp) && fs.statSync(fp).isFile()) return send(fs.readFileSync(fp), MIME[path.extname(fp)] ?? "application/octet-stream");
  send(indexHtml, "text/html"); // SPA fallback: every route serves the app shell
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

// Async on purpose: the page server above lives in this process, so a blocking spawn would
// deadlock (Chrome waits on the server, the server can't run while we block).
function runPage(name, query) {
  return new Promise((resolve, reject) => {
    const child = spawn(chrome, [
      "--headless=new", "--disable-gpu", "--no-sandbox", "--no-first-run",
      `--user-data-dir=${path.join(tmp, "profile-" + name)}`, "--virtual-time-budget=900000", "--dump-dom",
      `${base}/${name}.html${query}`,
    ], { stdio: ["ignore", "pipe", "pipe"] });
    let out = "", err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    const timer = setTimeout(() => { child.kill("SIGKILL"); reject(new Error(`${name}: Chrome timed out`)); }, 600000);
    child.on("close", () => {
      clearTimeout(timer);
      const m = /RESULT([\s\S]*?)END/.exec(out);
      if (!m) return reject(new Error(`${name}: no result from Chrome\n${err.slice(-500)}`));
      // dump-dom HTML-escapes the <pre> text; undo the entities JSON.stringify output can contain.
      const text = m[1].replace(/&quot;/g, '"').replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
      try { resolve(JSON.parse(text)); } catch (e) { reject(e); }
    });
  });
}

const failures = new Map(); // message -> Set of "route @width"
const fail = (msg, where) => { if (!failures.has(msg)) failures.set(msg, new Set()); failures.get(msg).add(where); };
const warnings = new Set();

try {
  const q = new URLSearchParams({ widths: process.env.LAYOUT_WIDTHS ?? PAGE_WIDTHS });
  if (process.env.LAYOUT_ROUTES) q.set("routes", process.env.LAYOUT_ROUTES);
  if (process.env.LAYOUT_PROBE) q.set("probe", process.env.LAYOUT_PROBE);

  console.log("layout: checking pages…");
  const pages = await runPage("pages", "?" + q);
  for (const r of pages) {
    if (r.probe) console.log(`probe ${r.route} @${r.width}: ${r.probe.join(" ")}`);
    const where = `${r.route} @${r.width}px`;
    if (r.error) { fail(`measurement error: ${r.error}`, where); continue; }
    if (r.crashed) fail("page crashed (ErrorBoundary shown)", where);
    if (r.textLen < MIN_PAGE_TEXT) fail(`page rendered almost nothing — broken mock, route, or a state the test must drive first. It shows: "${r.mainText}"`, where);
    for (const h of r.hscroll) fail(`horizontal scroll: ${h}${r.culprits?.length ? "  culprits: " + r.culprits.join(" | ") : ""}`, where);
    for (const c of r.clipped) fail(`clipped control: ${c}`, where);
    for (const c of r.squeezed ?? []) fail(`shrinkable column squeezed to almost nothing: ${c}`, where);
    for (const c of r.silent ?? []) fail(`content silently clipped by overflow:hidden: ${c}`, where);
    for (const w of r.wraps) if (!ALLOWED_WRAPS.some((re) => re.test(w))) fail(`short text wrapped: ${w}`, where);
    for (const u of r.unmocked ?? []) warnings.add(u);
  }

  console.log("layout: checking modals…");
  const mq = new URLSearchParams();
  if (process.env.LAYOUT_WIDTHS) mq.set("widths", process.env.LAYOUT_WIDTHS);
  const modals = await runPage("modals", mq.size ? "?" + mq : "");
  for (const r of modals) {
    const where = `${r.route} ${r.spec} @${r.width}px`;
    if (r.error) { fail(`measurement error: ${r.error}`, where); continue; }
    // A scenario that can't open its modal is a test that silently stopped testing.
    if (r.skipped) { fail(`modal scenario did not open (${r.skipped}) — UI changed? update modals.html`, where); continue; }
    if (!r.title) fail("modal has no title", where);
    if (!r.vfit) fail("modal taller than the window (doesn't fit at this height)", where);
    if (r.bodyHscroll > 0) fail(`modal body clips content horizontally (+${r.bodyHscroll}px)`, where);
    for (const c of r.clipped) fail(`element outside the dialog: ${c}`, where);
    for (const w of r.wraps) fail(`short text wrapped in modal: ${w}`, where);
  }
  console.log(`layout: ${pages.length} page checks, ${modals.length} modal checks`);
} finally {
  server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
}

if (warnings.size) console.log(`layout: note — unmocked commands returned null: ${[...warnings].filter((c) => !["set_menu_auth_state", "activate_tray", "deactivate_tray", "show_main_window"].includes(c)).join(", ") || "(startup-only)"}`);

if (failures.size) {
  console.error(`\nlayout: ${failures.size} distinct failure(s):\n`);
  for (const [msg, wheres] of failures) {
    const list = [...wheres];
    console.error(`  ✗ ${msg}\n      ${list.slice(0, 6).join("\n      ")}${list.length > 6 ? `\n      … and ${list.length - 6} more` : ""}`);
  }
  process.exit(1);
}
console.log("layout: all checks passed");
