import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { interactionState, interactionChanged } from "../ui_test.mjs";

const html = `<!doctype html><title>Interaction effects</title>
<button id="focus" onclick="document.getElementById('content').focus({preventScroll:true})">Move focus</button>
<button id="scroll" onclick="document.getElementById('panel').scrollTop=120">Scroll panel</button>
<button id="dead">Does nothing</button>
<a href="#content">Fragment link</a><main id="content" tabindex="-1">Main content</main>
<div id="panel" style="height:60px;overflow:auto"><div style="height:500px">Scrollable content</div></div>
<div style="height:1200px"></div><button id="offscreen">Offscreen does nothing</button>`;
const nativeJs = fs.readFileSync(new URL("../../src/native/web_page.js", import.meta.url), "utf8");
const nativeRust = fs.readFileSync(new URL("../../src/native/web.rs", import.meta.url), "utf8");
const nativeState = nativeRust.match(/const INTERACTION_STATE_JS: &str = r#"([\s\S]*?)"#;/)[1];

test("focus and scrolling are real effects while click focus and automatic scrolling are not", async () => {
  const server = http.createServer((_, res) => { res.writeHead(200, { "content-type": "text/html" }); res.end(html); });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}/`;
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "kritic-effects-"));
  let browser;
  try {
    // First reproduce the native detector's blind spot using its actual page helper.
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    await page.goto(base);
    await page.evaluate(nativeJs);
    await page.evaluate(() => window.__kritic.collect({}));
    const before = await page.evaluate(() => window.__kritic.signature());
    const key = await page.locator("#focus").getAttribute("data-kritic-id");
    const focusBefore = await page.evaluate(interactionState, key);
    assert.deepEqual(await page.evaluate(`(${nativeState})(${JSON.stringify(key)})`), focusBefore);
    await page.locator("#focus").click();
    assert.equal(await page.evaluate(() => document.activeElement.id), "content");
    assert.deepEqual(await page.evaluate(() => window.__kritic.signature()), before, "focus does not mutate DOM or storage");
    const focusAfter = await page.evaluate(interactionState, key);
    assert.deepEqual(await page.evaluate(`(${nativeState})(${JSON.stringify(key)})`), focusAfter);
    assert.equal(interactionChanged(focusBefore, focusAfter), true);
    await browser.close(); browser = null;
    const harness = fileURLToPath(new URL("../ui_test.mjs", import.meta.url));
    const result = await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, [harness], { stdio: ["pipe", "pipe", "inherit"] });
      let out = "";
      child.stdout.on("data", b => { out += b; });
      child.on("error", reject);
      child.on("close", () => { try { resolve(JSON.parse(out)); } catch (e) { reject(e); } });
      child.stdin.end(JSON.stringify({ url: base, out_dir: outDir, axe: false, max_pages: 1, wait_ms: 20 }));
    });
    assert.equal(result.ok, true, result.error);
    const byName = Object.fromEntries(result.interactions.map(i => [i.element.name, i.result]));
    assert.equal(byName["Move focus"], "ok", JSON.stringify(byName));
    assert.equal(byName["Scroll panel"], "ok", JSON.stringify(byName));
    assert.equal(byName["Does nothing"], "no_effect");
    assert.equal(byName["Offscreen does nothing"], "no_effect");
    assert.equal(byName["Fragment link"], "navigated");
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
    fs.rmSync(outDir, { recursive: true, force: true });
  }
});
