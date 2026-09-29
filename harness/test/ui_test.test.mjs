// node --test harness/test/  (needs PLAYWRIGHT_BROWSERS_PATH pointing at the installed Chromium)
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const fixtures = path.join(here, "fixtures");
const harness = path.join(here, "..", "ui_test.mjs");
let server;
let base;

before(async () => {
  server = http.createServer((req, res) => {
    const p = path.join(fixtures, decodeURIComponent(new URL(req.url, "http://x").pathname));
    if (p.startsWith(fixtures) && fs.existsSync(p) && fs.statSync(p).isFile()) {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      fs.createReadStream(p).pipe(res);
    } else {
      res.writeHead(404);
      res.end("not found");
    }
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${server.address().port}`;
});
after(() => server.close());

function run(input) {
  return new Promise((resolve, reject) => {
    const p = spawn(process.execPath, [harness], { stdio: ["pipe", "pipe", "inherit"] });
    let out = "";
    p.stdout.on("data", (d) => (out += d));
    p.on("close", () => {
      try { resolve(JSON.parse(out)); } catch (e) { reject(new Error(`bad output: ${out.slice(0, 300)}`)); }
    });
    p.stdin.end(JSON.stringify(input));
  });
}

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), "kritic-ui-"));

test("good app: no errors, persistence holds, links crawled", async () => {
  const r = await run({
    url: `${base}/good.html`, out_dir: tmp(),
    persist: [{ selector: "#name" }, { selector: "#dark" }, { selector: "#size" }],
  });
  assert.equal(r.ok, true, r.error);
  assert.deepEqual(r.page_errors, []);
  assert.deepEqual(r.console_errors, []);
  assert.deepEqual(r.failed_requests.filter((f) => !f.url.endsWith("favicon.ico")), []);
  assert.ok(r.pages.some((p) => p.url.endsWith("/about.html")), "crawled about page");
  assert.ok(r.persistence.every((p) => p.persisted), JSON.stringify(r.persistence));
  const save = r.interactions.find((i) => i.element.name === "Save");
  assert.equal(save.result, "ok");
  assert.ok(r.pages.every((p) => fs.existsSync(p.screenshot)));
  assert.ok(r.storage_keys_written.includes("name"));
});

test("broken app: every planted defect is reported", async () => {
  const r = await run({ url: `${base}/broken.html`, out_dir: tmp(), persist: [{ selector: "#note" }] });
  assert.equal(r.ok, true, r.error);
  const by = (name) => r.interactions.find((i) => i.element.name === name);
  assert.equal(by("Throw").result, "error");
  assert.match(by("Throw").errors.join(" "), /doesNotExist/);
  assert.equal(by("Log error").result, "error");
  assert.equal(by("Does nothing").result, "no_effect");
  assert.equal(by("Delete account").result, "skipped");
  assert.equal(by("External").result, "skipped");
  assert.equal(by("Good page").result, "navigated");
  assert.ok(r.failed_requests.some((f) => f.url.endsWith("/api/missing") && f.status === 404));
  assert.ok(r.failed_requests.some((f) => f.url.endsWith("/logo.png")));
  assert.equal(r.persistence[0].persisted, false);
  const axe = r.pages[0].axe.map((v) => v.id);
  assert.ok(axe.includes("image-alt"), axe.join(","));
  assert.ok(axe.includes("color-contrast"), axe.join(","));
});

test("refuses non-local URLs unless allowed", async () => {
  const r = await run({ url: "https://example.com/", out_dir: tmp() });
  assert.equal(r.ok, false);
  assert.match(r.error, /non-local/);
});

test("screenshot mode writes a PNG", async () => {
  const out = path.join(tmp(), "shot.png");
  const r = await run({ mode: "screenshot", url: `${base}/good.html`, out_path: out });
  assert.equal(r.ok, true, r.error);
  assert.ok(fs.statSync(out).size > 1000);
});

test("flows: passing multi-step flow and a failing one that stops at the right step", async () => {
  const r = await run({
    url: `${base}/good.html`, out_dir: tmp(), explore: false,
    flows: [
      { name: "save profile", steps: [
        { do: "fill", target: "#name", value: "Ada" },
        { do: "select", target: "#size", value: "Large" },
        { do: "click", target: "text=Save" },
        { do: "expect_text", target: "#status", value: "Saved" },
        { do: "click", target: "text=About" },
        { do: "expect_url", value: "/about.html" },
      ] },
      { name: "broken throw", url: "/broken.html", steps: [
        { do: "click", target: "#crashy" },
        { do: "click", target: "text=Good page" },
      ] },
      { name: "missing element", steps: [
        { do: "click", target: "#does-not-exist", timeout_ms: 800 },
        { do: "click", target: "text=Save" },
      ] },
    ],
  });
  assert.equal(r.ok, true, r.error);
  assert.equal(r.pages.length, 0, "explore:false skips the crawl");
  const [ok, broken, missing] = r.flows;
  assert.equal(ok.passed, true, JSON.stringify(ok.steps));
  assert.equal(ok.steps.length, 6);
  assert.equal(broken.passed, false);
  assert.match(broken.steps[0].error, /doesNotExist/);
  assert.equal(broken.steps[1].result, "skipped");
  assert.equal(missing.steps[0].result, "fail");
  assert.ok(fs.existsSync(missing.steps[0].screenshot));
});
