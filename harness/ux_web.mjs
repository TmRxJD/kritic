// ux_critic web adapter: a persistent Playwright session driven by kritic over JSON lines.
// stdin: one request per line; stdout: one reply per line; stderr: progress events.
//
//   {op:"start", url, viewport?, allow_remote?, allow_destructive?, headed?, settle_ms?, ignore?: [regex]}
//   {op:"reset"}                                   fresh context (no storage), load url
//   {op:"observe", shot?, marks?, axe?}            -> {obs: {fingerprint, url, title, text, viewport,
//                                                       controls:[{id,label,text,tooltip,role,rect,enabled,inline,selected,skip?}],
//                                                       screenshot?, marked?, axe?}}
//   {op:"act", id}                                 click / fill / select the control with that id
//   {op:"close"}
//
// Control ids are stable across replays: role, label and the index among controls sharing both.
import { chromium } from "playwright";
import AxeBuilder from "@axe-core/playwright";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { DESTRUCTIVE, collectCandidates, isLocal } from "./ui_test.mjs";

const waitMs = () => cfg.settle_ms ?? 150; // after each load or action
let browser = null;
let context = null;
let page = null;
let cfg = {};
let inflight = 0;
let current = new Map(); // control id -> data-kritic-id, refreshed by mapControls

const reply = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");

// Settled: the settle time has passed and no request has been in flight for 50 ms (at most 3 s).
// Playwright's "networkidle" needs 500 ms of quiet, which alone made every reset cost half a second.
async function settle() {
  await page.waitForTimeout(waitMs());
  const until = Date.now() + 3000;
  let quietSince = 0;
  while (Date.now() < until) {
    if (inflight) quietSince = 0;
    else if (!quietSince) quietSince = Date.now();
    else if (Date.now() - quietSince >= 50) return;
    await page.waitForTimeout(10);
  }
}

function roleOf(c) {
  if (c.role) return c.role;
  if (c.tag === "a") return "link";
  if (c.tag === "select") return "combobox";
  if (c.tag === "textarea") return "textbox";
  if (c.tag === "input") {
    if (["checkbox", "radio"].includes(c.type)) return c.type;
    if (["submit", "button", "reset", "image"].includes(c.type)) return "button";
    return "textbox";
  }
  if (c.tag === "summary") return "button";
  return c.tag;
}

async function start(req) {
  const u = new URL(req.url);
  if (!req.allow_remote && !isLocal(u)) throw new Error(`refusing non-local URL ${u.origin} (set allow_remote)`);
  cfg = req;
  if (browser) await browser.close().catch(() => {}); // a second start replaces the session
  context = page = null;
  browser = await chromium.launch({ headless: !req.headed });
  return { browser_version: browser.version() };
}

async function reset() {
  if (context) await context.close().catch(() => {});
  context = await browser.newContext({ viewport: cfg.viewport || { width: 1280, height: 800 } });
  context.setDefaultTimeout(10000);
  page = await context.newPage();
  page.on("dialog", (d) => d.dismiss().catch(() => {}));
  page.on("popup", (w) => w.close().catch(() => {}));
  inflight = 0;
  page.on("request", () => inflight++);
  for (const done of ["requestfinished", "requestfailed"]) page.on(done, () => { inflight = Math.max(0, inflight - 1); });
  await page.goto(cfg.url, { waitUntil: "load" });
  await settle();
  current = new Map();
  return {};
}

// A screen is what a user sees: where they are, which controls are there (and their state), and
// the visible text with digits folded, so a ticking clock or counter is not a new screen.
function fingerprint(url, controls, text) {
  const u = new URL(url);
  const sig = [u.pathname + u.hash, ...controls.map((c) => `${c.id}|${c.enabled}|${c.selected}`),
    text.replace(/\d+/g, "0")].join("\n");
  return crypto.createHash("sha1").update(sig).digest("hex").slice(0, 16);
}

async function drawMarks(controls) {
  await page.evaluate((marks) => {
    const layer = document.createElement("div");
    layer.id = "__kritic_marks";
    layer.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647";
    for (const m of marks) {
      const box = document.createElement("div");
      box.style.cssText = `position:fixed;left:${m.x}px;top:${m.y}px;width:${m.w}px;height:${m.h}px;` +
        "outline:2px solid #e00;box-sizing:border-box";
      const tag = document.createElement("div");
      tag.textContent = String(m.n);
      tag.style.cssText = `position:fixed;left:${Math.max(0, m.x - 2)}px;top:${Math.max(0, m.y - 14)}px;` +
        "background:#e00;color:#fff;font:bold 12px/14px sans-serif;padding:0 3px;border-radius:2px";
      layer.append(box, tag);
    }
    document.body.append(layer);
  }, controls.map((c, i) => ({ n: i + 1, x: c.rect[0], y: c.rect[1], w: c.rect[2], h: c.rect[3] })));
}

// Every control on the screen with its stable id; refreshes the id -> element map.
async function mapControls() {
  const raw = await page.evaluate(collectCandidates, { all: true });
  const start = new URL(cfg.url);
  const seen = new Map();
  current = new Map();
  const controls = raw.map((c) => {
    const role = roleOf(c);
    const key = `${role}:${c.label || c.text}`;
    const n = (seen.get(key) || 0) + 1;
    seen.set(key, n);
    const id = `${key}#${n}`;
    current.set(id, c.kritic);
    const out = { id, label: c.label, text: c.text, tooltip: c.tooltip, role, rect: c.rect,
      enabled: c.enabled, inline: c.inline, selected: c.selected };
    if ((cfg.ignore || []).some((rx) => new RegExp(rx).test(c.label))) out.skip = "ignored";
    else if (!cfg.allow_destructive && DESTRUCTIVE.test(`${c.label} ${c.hint} ${c.href}`)) out.skip = "destructive";
    else if (c.type === "file") out.skip = "file input";
    else if (c.href && !c.href.startsWith("javascript:")) {
      try {
        if (new URL(c.href).origin !== start.origin) out.skip = "external link";
      } catch { /* relative or odd href: same page */ }
    }
    return out;
  });
  return controls;
}

async function observe(req) {
  const controls = await mapControls();
  const text = await page.evaluate(() => (document.body ? document.body.innerText : "")).catch(() => "");
  const vp = page.viewportSize() || { width: 1280, height: 800 };
  const obs = {
    fingerprint: fingerprint(page.url(), controls, text), url: page.url(), title: await page.title(),
    text: text.replace(/\s+/g, " ").slice(0, 20000), viewport: [vp.width, vp.height], controls,
  };
  if (req.axe) {
    try {
      const res = await new AxeBuilder({ page }).withRules(["color-contrast"]).analyze();
      obs.axe = res.violations.map((v) => ({ id: v.id, impact: v.impact, help: v.help, nodes: v.nodes.length,
        targets: v.nodes.slice(0, 3).map((n) => n.target.join(" ")) }));
    } catch (e) {
      obs.axe_error = String(e.message).slice(0, 200);
    }
  }
  if (req.shot) {
    fs.mkdirSync(path.dirname(req.shot), { recursive: true });
    await page.screenshot({ path: req.shot });
    obs.screenshot = req.shot;
  }
  if (req.marks) {
    fs.mkdirSync(path.dirname(req.marks), { recursive: true });
    // numbered in the order kritic lists the actionable controls to the novice: enabled, not skipped
    await drawMarks(controls.filter((c) => c.enabled && !c.skip));
    await page.screenshot({ path: req.marks });
    await page.evaluate(() => document.getElementById("__kritic_marks")?.remove());
    obs.marked = req.marks;
  }
  return { obs };
}

async function act(req) {
  await mapControls(); // replays act without observing in between
  const kritic = current.get(req.id);
  if (kritic === undefined) throw new Error(`no control ${req.id} on this screen`);
  const loc = page.locator(`[data-kritic-id="${kritic}"]`);
  const info = await loc.evaluate((el) => ({ tag: el.tagName.toLowerCase(), type: (el.type || "").toLowerCase() }));
  let action = "click";
  if (info.tag === "select") {
    action = "select";
    const n = await loc.locator("option").count();
    await loc.selectOption({ index: n > 1 ? 1 : 0 });
  } else if (info.tag === "textarea" || info.tag === "input" && !["checkbox", "radio", "submit", "button", "reset", "image"].includes(info.type)) {
    action = "fill";
    await loc.fill("kritic test");
    await loc.press("Tab").catch(() => {});
  } else {
    await loc.click();
  }
  await settle();
  return { action };
}

const ops = { start, reset, observe, act };

async function main() {
  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of rl) {
    if (!line.trim()) continue;
    let req;
    try {
      req = JSON.parse(line);
    } catch (e) {
      reply({ ok: false, error: `bad request JSON: ${e.message}` });
      continue;
    }
    if (req.op === "close") break;
    try {
      const fn = ops[req.op];
      if (!fn) throw new Error(`unknown op ${req.op}`);
      if (req.op !== "start" && !browser) throw new Error("start first");
      if (!["start", "reset"].includes(req.op) && !page) throw new Error("reset first");
      reply({ ok: true, ...(await fn(req)) });
    } catch (e) {
      reply({ ok: false, error: String(e.message).split("\n")[0].slice(0, 300) });
    }
  }
  // stdin closed or close requested: the browser (and its child processes) must not outlive us
  if (browser) await browser.close().catch(() => {});
  reply({ ok: true, closed: true });
  process.exit(0);
}

main();
