// Deterministic UI tester for kritic. No LLM involvement: every result is an observation.
// Input: one JSON object on stdin. Output: one JSON object on stdout.
//
//   mode "ui_test":    crawl same-origin pages, exercise every visible control, collect errors,
//                      no-effect controls, persistence failures, axe-core violations, screenshots.
//   mode "screenshot": capture one page to a PNG (used by visual_critic).
import { chromium } from "playwright";
import AxeBuilder from "@axe-core/playwright";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const DESTRUCTIVE =
  /\b(delete|remove|destroy|drop|erase|wipe|reset all|log ?out|sign ?out|unsubscribe|purchase|buy|pay|checkout|deactivate|close account)\b/i;
const LOCAL_HOST = /^(localhost|127\.0\.0\.1|\[::1\]|.+\.localhost|.+\.test)$/i;

function readStdin() {
  return new Promise((resolve, reject) => {
    let data = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (c) => (data += c));
    process.stdin.on("end", () => resolve(data));
    process.stdin.on("error", reject);
  });
}

// One JSON line on stderr per step; the MCP server streams these to its WebSocket.
function progress(event, data = {}) {
  process.stderr.write(JSON.stringify({ event, ...data }) + "\n");
}

export function isLocal(u) {
  return u.protocol === "file:" || LOCAL_HOST.test(u.hostname);
}

function testValue(type) {
  switch (type) {
    case "email": return "kritic@example.test";
    case "number": case "range": return "42";
    case "url": return "https://example.test";
    case "tel": return "5550100";
    case "date": return "2026-01-01";
    case "time": return "12:00";
    case "datetime-local": return "2026-01-01T12:00";
    case "month": return "2026-01";
    case "week": return "2026-W01";
    case "color": return "#336699";
    case "password": return "kritic-test-pass";
    default: return "kritic test";
  }
}

// Runs in the page: tag candidates with data-kritic-id and describe them. `all` (ux_critic) also
// returns disabled controls and what a user perceives of each: accessible label, visible text,
// tooltip, on-screen rect, whether it sits inline in text, and whether it shows as selected.
export function collectCandidates(opts = {}) {
  for (const el of document.querySelectorAll("[data-kritic-id]")) el.removeAttribute("data-kritic-id");
  const sel = [
    "button", "[role=button]", "[role=link]", "[role=tab]", "[role=switch]", "[role=checkbox]",
    "a[href]", "summary", "input:not([type=hidden])", "textarea", "select", "[onclick]", "[contenteditable=true]",
  ].join(",");
  const out = [];
  let n = 0;
  for (const el of document.querySelectorAll(sel)) {
    const r = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    const visible = r.width > 0 && r.height > 0 && style.visibility !== "hidden" && style.display !== "none";
    const disabled = el.disabled || el.getAttribute("aria-disabled") === "true";
    if (!visible || (disabled && !opts.all)) continue;
    if (el.closest("[data-kritic-id]") && el.closest("[data-kritic-id]") !== el) continue; // nested in a control
    const id = String(n++);
    el.setAttribute("data-kritic-id", id);
    const labelText = el.labels && el.labels.length ? el.labels[0].innerText : "";
    const isField = ["INPUT", "SELECT", "TEXTAREA"].includes(el.tagName);
    const name = (
      el.getAttribute("aria-label") || labelText || (isField ? "" : el.innerText) ||
      el.getAttribute("placeholder") || (el.tagName === "INPUT" && ["submit", "button"].includes(el.type) ? el.value : "") ||
      el.getAttribute("name") || el.id || el.getAttribute("title") || el.innerText || ""
    ).trim().replace(/\s+/g, " ").slice(0, 80);
    const c = {
      kritic: id,
      tag: el.tagName.toLowerCase(),
      type: (el.getAttribute("type") || "").toLowerCase(),
      role: el.getAttribute("role") || "",
      name,
      href: el.tagName === "A" ? el.href : "",
      hint: [el.id, el.className && typeof el.className === "string" ? el.className : ""].join(" "),
    };
    if (opts.all) {
      const byIds = (attr) => (el.getAttribute(attr) || "").split(/\s+/).filter(Boolean)
        .map((x) => document.getElementById(x)?.innerText || "").join(" ");
      const text = (isField ? "" : el.innerText || "").trim();
      const alt = [...el.querySelectorAll("img[alt]")].map((i) => i.alt).join(" ");
      c.label = (el.getAttribute("aria-label") || byIds("aria-labelledby") || labelText || text || alt ||
        (el.tagName === "INPUT" && ["submit", "button"].includes(el.type) ? el.value : "") ||
        el.getAttribute("placeholder") || el.getAttribute("title") || "").trim().replace(/\s+/g, " ").slice(0, 80);
      c.text = text.replace(/\s+/g, " ").slice(0, 80);
      c.tooltip = (el.getAttribute("title") || byIds("aria-describedby")).trim().slice(0, 160);
      c.rect = [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)];
      c.enabled = !disabled;
      c.inline = el.tagName === "A" && style.display === "inline";
      c.selected = el.getAttribute("aria-selected") === "true" || el.getAttribute("aria-pressed") === "true" ||
        !!el.getAttribute("aria-current") && el.getAttribute("aria-current") !== "false" || !!el.checked;
    }
    out.push(c);
  }
  return out;
}

function pageSignature() {
  const s = document.body ? document.body.innerHTML : "";
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  let store = "";
  try {
    store = JSON.stringify(Object.entries(localStorage)) + JSON.stringify(Object.entries(sessionStorage));
  } catch { /* storage may be blocked */ }
  return { dom: h, len: s.length, store };
}

async function settle(page, ms) {
  await page.waitForTimeout(ms);
  await page.waitForLoadState("networkidle", { timeout: 2000 }).catch(() => {});
}

// Snapshot after the tester scrolls the target into view. Focusing the clicked control is
// native click behavior, not evidence its action worked; focus moved elsewhere is observable.
export function interactionState(key) {
  const target = document.querySelector(`[data-kritic-id="${key}"]`);
  const elements = [...document.querySelectorAll("*")];
  const active = document.activeElement;
  return {
    focus: elements.indexOf(active),
    redirected: !!active && active !== document.body && active !== document.documentElement && active !== target,
    scroll: [[-1, window.scrollX, window.scrollY], ...elements.flatMap((el, i) =>
      el.scrollHeight > el.clientHeight || el.scrollWidth > el.clientWidth
        ? [[i, el.scrollLeft, el.scrollTop]] : [])],
  };
}

export function interactionChanged(before, after) {
  return !!before && !!after && (JSON.stringify(before.scroll) !== JSON.stringify(after.scroll) ||
    after.redirected && before.focus !== after.focus);
}

async function screenshot(input) {
  const u = new URL(input.url);
  if (!input.allow_remote && !isLocal(u)) throw new Error(`refusing non-local URL ${u.origin} (set allow_remote)`);
  const browser = await chromium.launch({ headless: !input.headed });
  try {
    const page = await browser.newPage({ viewport: input.viewport || { width: 1280, height: 800 } });
    const resp = await page.goto(input.url, { waitUntil: "load", timeout: input.timeout_ms || 15000 });
    await settle(page, 300);
    fs.mkdirSync(path.dirname(input.out_path), { recursive: true });
    await page.screenshot({ path: input.out_path, fullPage: input.full_page !== false });
    return { ok: true, path: input.out_path, status: resp ? resp.status() : null, title: await page.title() };
  } finally {
    await browser.close();
  }
}

async function uiTest(input) {
  if (input.flows != null && !Array.isArray(input.flows)) throw new Error("flows must be an array");
  input.flows = (input.flows || []).map((raw, i) => {
    const flow = typeof raw === "string" ? JSON.parse(raw) : raw;
    if (!flow || typeof flow !== "object" || Array.isArray(flow)) throw new Error(`flow ${i + 1}: must be an object`);
    if (!Array.isArray(flow.steps) || flow.steps.length === 0) throw new Error(`flow ${i + 1}: steps must be a non-empty array`);
    if (flow.steps.some(s => !s || typeof s !== "object" || typeof s.do !== "string" || !s.do.length)) throw new Error(`flow ${i + 1}: each step must be an object with a non-empty do action`);
    return flow;
  });
  const start = new URL(input.url);
  if (!input.allow_remote && !isLocal(start)) throw new Error(`refusing non-local URL ${start.origin} (set allow_remote)`);
  const maxPages = input.max_pages ?? 5;
  const maxActions = input.max_actions ?? 60;
  const waitMs = input.wait_ms ?? 250;
  const timeout = input.timeout_ms ?? 10000;
  const outDir = input.out_dir || path.join(process.cwd(), "out");
  fs.mkdirSync(outDir, { recursive: true });

  const r = {
    ok: true, url: input.url, browser_version: "", pages: [], interactions: [],
    console_errors: [], page_errors: [], failed_requests: [], crashes: [], dialogs: [], popups: [],
    persistence: [], storage_keys_written: [],
  };
  const browser = await chromium.launch({ headless: !input.headed });
  r.browser_version = browser.version();
  const context = await browser.newContext({ viewport: input.viewport || { width: 1280, height: 800 } });
  context.setDefaultTimeout(timeout);
  // Popups opened by the app under test are recorded and closed. (Don't watch context "page"
  // events: axe-core opens its own helper page in the context.)
  const openPage = () => context.newPage();
  let page = await openPage();
  let current = "";
  let requests = 0;
  const wire = (p) => {
    p.on("console", (m) => {
      if (m.type() === "error") r.console_errors.push({ page: current, text: m.text().slice(0, 500) });
    });
    p.on("pageerror", (e) => r.page_errors.push({ page: current, message: String(e.message).slice(0, 500) }));
    p.on("request", () => requests++);
    p.on("requestfailed", (q) =>
      r.failed_requests.push({ page: current, url: q.url(), failure: q.failure()?.errorText || "failed" }));
    p.on("response", (s) => {
      if (s.status() >= 400) r.failed_requests.push({ page: current, url: s.url(), status: s.status() });
    });
    p.on("crash", () => r.crashes.push({ page: current }));
    p.on("popup", async (w) => {
      r.popups.push({ page: current, url: w.url() });
      await w.close().catch(() => {});
    });
    p.on("dialog", async (d) => {
      r.dialogs.push({ page: current, type: d.type(), message: d.message().slice(0, 200) });
      await d.dismiss().catch(() => {});
    });
  };
  wire(page);

  const queue = [start.href, ...(input.routes || []).map((x) => new URL(x, start).href)];
  const seen = new Set();
  let actions = 0;
  const storageBefore = new Set();

  if (input.explore === false) queue.length = 0;
  while (queue.length && r.pages.length < maxPages) {
    const url = queue.shift();
    const key = url.split("#")[0];
    if (seen.has(key)) continue;
    seen.add(key);
    current = url;
    const t0 = Date.now();
    let status = null;
    try {
      const resp = await page.goto(url, { waitUntil: "load" });
      status = resp ? resp.status() : null;
    } catch (e) {
      r.pages.push({ url, status: null, error: String(e.message).slice(0, 300) });
      continue;
    }
    await settle(page, waitMs);
    const pageRec = { url, status, title: await page.title(), load_ms: Date.now() - t0 };
    progress("page", { url, status, load_ms: pageRec.load_ms, n: r.pages.length + 1 });
    if (r.pages.length === 0) {
      // baseline: keys present before the tester touched anything
      try {
        for (const k of await page.evaluate(() => Object.keys(localStorage))) storageBefore.add(k);
      } catch { /* ignore */ }
    }

    if (input.axe !== false) {
      try {
        const res = await new AxeBuilder({ page }).analyze();
        pageRec.axe = res.violations.map((v) => ({
          id: v.id, impact: v.impact, help: v.help, nodes: v.nodes.length,
          targets: v.nodes.slice(0, 3).map((n) => n.target.join(" ")),
        }));
      } catch (e) {
        pageRec.axe_error = String(e.message).slice(0, 200);
      }
    }
    const shot = path.join(outDir, `page-${r.pages.length}.png`);
    await page.screenshot({ path: shot, fullPage: true }).catch(() => {});
    pageRec.screenshot = shot;
    r.pages.push(pageRec);

    // discover same-origin links
    const links = await page.$$eval("a[href]", (as) => as.map((a) => a.href)).catch(() => []);
    for (const l of links) {
      try {
        const u = new URL(l);
        if (u.origin === start.origin && /^https?:|^file:/.test(u.protocol)) queue.push(u.href.split("#")[0]);
      } catch { /* ignore bad hrefs */ }
    }

    const cands = await page.evaluate(collectCandidates);
    for (const c of cands) {
      if (actions >= maxActions) break;
      const rec = { page: url, element: { tag: c.tag, type: c.type, role: c.role, name: c.name }, action: "", result: "" };
      const label = `${c.name} ${c.hint} ${c.href}`;
      if (!input.allow_destructive && DESTRUCTIVE.test(label)) {
        r.interactions.push({ ...rec, action: "none", result: "skipped", detail: "looks destructive (set allow_destructive)" });
        continue;
      }
      if (c.tag === "a" && c.href) {
        try {
          const u = new URL(c.href);
          if (u.origin !== start.origin && !u.href.startsWith("javascript:")) {
            r.interactions.push({ ...rec, action: "none", result: "skipped", detail: "external link" });
            continue;
          }
        } catch { /* fall through */ }
      }
      if (c.type === "file") {
        r.interactions.push({ ...rec, action: "none", result: "skipped", detail: "file input" });
        continue;
      }
      if (page.url().split("#")[0] !== url.split("#")[0]) {
        await page.goto(url, { waitUntil: "load" }).catch(() => {});
        await settle(page, waitMs);
        await page.evaluate(collectCandidates).catch(() => {});
      }
      const loc = page.locator(`[data-kritic-id="${c.kritic}"]`);
      if ((await loc.count()) === 0) {
        r.interactions.push({ ...rec, action: "none", result: "skipped", detail: "element gone after earlier interaction" });
        continue;
      }
      actions++;
      const consoleBefore = r.console_errors.length;
      const pageErrBefore = r.page_errors.length;
      const crashesBefore = r.crashes.length;
      // Exclude Playwright's preparatory viewport movement from the action's effects.
      await loc.scrollIntoViewIfNeeded();
      const effectBefore = await page.evaluate(interactionState, c.kritic).catch(() => null);
      const before = await page.evaluate(pageSignature).catch(() => null);
      const reqBefore = requests;
      const urlBefore = page.url();
      try {
        if (c.tag === "select") {
          rec.action = "select";
          const opts = await loc.locator("option").count();
          await loc.selectOption({ index: opts > 1 ? 1 : 0 });
        } else if (c.tag === "textarea" || c.tag === "input" && !["checkbox", "radio", "submit", "button", "reset", "image"].includes(c.type)) {
          rec.action = "fill";
          await loc.fill(testValue(c.type));
          await loc.press("Tab").catch(() => {});
        } else if (c.type === "checkbox" || c.type === "radio" || c.role === "checkbox" || c.role === "switch") {
          rec.action = "toggle";
          await loc.click();
        } else if (c.tag === "input" && c.type === "reset") {
          rec.action = "click";
          await loc.click();
        } else {
          rec.action = "click";
          await loc.click();
        }
        await settle(page, waitMs);
        const after = await page.evaluate(pageSignature).catch(() => null);
        const effectAfter = await page.evaluate(interactionState, c.kritic).catch(() => null);
        const navigated = page.url() !== urlBefore;
        const newErrors = [...r.page_errors.slice(pageErrBefore), ...r.console_errors.slice(consoleBefore)];
        if (r.crashes.length > crashesBefore) {
          rec.result = "crash";
          page = await openPage();
          wire(page);
        } else if (newErrors.length) {
          rec.result = "error";
          rec.errors = newErrors.map((e) => e.text || e.message);
        } else if (navigated) {
          rec.result = "navigated";
          rec.detail = page.url();
        } else if (rec.action === "click" && before && after && before.dom === after.dom &&
                   before.store === after.store && requests === reqBefore &&
                   !interactionChanged(effectBefore, effectAfter)) {
          rec.result = "no_effect";
        } else {
          rec.result = "ok";
        }
      } catch (e) {
        rec.result = "error";
        rec.errors = [String(e.message).split("\n")[0].slice(0, 300)];
      }
      r.interactions.push(rec);
      progress("interaction", { n: actions, name: c.name, tag: c.tag, action: rec.action, result: rec.result });
    }
  }

  if ((input.flows || []).length) await runFlows(context, input.flows, start, outDir, r);

  // persistence: set each selector, reload, compare
  for (const p of input.persist || []) {
    const url = p.url ? new URL(p.url, start).href : start.href;
    current = url;
    const rec = { selector: p.selector, url };
    try {
      await page.goto(url, { waitUntil: "load" });
      await settle(page, waitMs);
      const loc = page.locator(p.selector).first();
      const kind = await loc.evaluate((el) => el.tagName.toLowerCase() + ":" + (el.type || ""));
      let expected;
      if (kind.endsWith(":checkbox") || kind.endsWith(":radio")) {
        expected = !(await loc.isChecked());
        await loc.setChecked(expected);
      } else if (kind.startsWith("select")) {
        const n = await loc.locator("option").count();
        await loc.selectOption({ index: n > 1 ? n - 1 : 0 });
        expected = await loc.inputValue();
      } else {
        expected = `kritic-persist-${Date.now() % 100000}`;
        await loc.fill(expected);
        await loc.press("Tab").catch(() => {});
      }
      await settle(page, waitMs);
      await page.reload({ waitUntil: "load" });
      await settle(page, waitMs);
      const loc2 = page.locator(p.selector).first();
      const actual = typeof expected === "boolean" ? await loc2.isChecked() : await loc2.inputValue();
      Object.assign(rec, { expected, actual, persisted: actual === expected });
    } catch (e) {
      Object.assign(rec, { persisted: false, error: String(e.message).split("\n")[0].slice(0, 300) });
    }
    r.persistence.push(rec);
  }

  try {
    await page.goto(start.href, { waitUntil: "load" });
    const keys = await page.evaluate(() => Object.keys(localStorage));
    r.storage_keys_written = keys.filter((k) => !storageBefore.has(k));
  } catch { /* ignore */ }
  await browser.close();
  const flowPages = (r.flows || []).filter(f => f.page_opened).length;
  const flowSteps = (r.flows || []).flatMap(f => f.steps).filter(s => s.do !== "open").length;
  progress("done", { pages: r.pages.length + flowPages, interactions: r.interactions.length + flowSteps });
  // One entry per URL; an HTTP status beats the ERR_ABORTED that follows navigating away.
  const byUrl = new Map();
  for (const f of r.failed_requests) {
    const prev = byUrl.get(f.url);
    if (!prev || (prev.status === undefined && f.status !== undefined)) byUrl.set(f.url, { ...f, count: (prev?.count || 0) + 1 });
    else prev.count++;
  }
  r.failed_requests = [...byUrl.values()];
  return r;
}

// ------------------------------------------------------------------ multi-step flows
//
// flow: {name, url?, steps: [step], continue_on_error?}
// step: {do, target?, value?, timeout_ms?}
//   do: navigate(value=url) | click | dblclick | fill | select | check | uncheck | press(value=key)
//       | hover | wait_for (target visible) | wait(value=ms) | expect_text(value substring)
//       | expect_visible | expect_hidden | expect_url(value substring) | screenshot
//   target: any Playwright selector (css, "text=Save", "role=button[name='Save']", ...)
async function runStep(page, step, base, outDir, shotName) {
  const t = step.timeout_ms ?? 5000;
  const loc = step.target ? page.locator(step.target).first() : null;
  const need = () => {
    if (!loc) throw new Error(`step "${step.do}" needs a target`);
    return loc;
  };
  switch (step.do) {
    case "navigate": await page.goto(new URL(step.value, base).href, { waitUntil: "load", timeout: t }); break;
    case "click": await need().click({ timeout: t }); break;
    case "dblclick": await need().dblclick({ timeout: t }); break;
    case "fill": await need().fill(String(step.value ?? ""), { timeout: t }); break;
    case "select": await need().selectOption(String(step.value), { timeout: t }); break;
    case "check": await need().check({ timeout: t }); break;
    case "uncheck": await need().uncheck({ timeout: t }); break;
    case "press": await (loc ? loc.press(step.value, { timeout: t }) : page.keyboard.press(step.value)); break;
    case "hover": await need().hover({ timeout: t }); break;
    case "wait_for": await need().waitFor({ state: "visible", timeout: t }); break;
    case "wait": await page.waitForTimeout(Number(step.value ?? 500)); break;
    case "expect_visible": await need().waitFor({ state: "visible", timeout: t }); break;
    case "expect_hidden": await need().waitFor({ state: "hidden", timeout: t }); break;
    case "expect_text": {
      const deadline = Date.now() + t;
      let text = "";
      while (Date.now() < deadline) {
        text = (await need().textContent({ timeout: t }).catch(() => "")) || (await need().inputValue().catch(() => ""));
        if (text.includes(String(step.value))) break;
        await page.waitForTimeout(100);
      }
      if (!text.includes(String(step.value))) throw new Error(`expected text containing "${step.value}", got "${text.slice(0, 120)}"`);
      break;
    }
    case "expect_url":
      if (!page.url().includes(String(step.value))) throw new Error(`expected URL containing "${step.value}", got ${page.url()}`);
      break;
    case "screenshot": {
      const p = path.join(outDir, `${shotName}.png`);
      await page.screenshot({ path: p, fullPage: true });
      return p;
    }
    default: throw new Error(`unknown step "${step.do}"`);
  }
  return null;
}

async function runFlows(context, flows, start, outDir, r) {
  const results = [];
  for (const [fi, flow] of flows.entries()) {
    const page = await context.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(`page error: ${String(e.message).slice(0, 300)}`));
    page.on("console", (m) => { if (m.type() === "error") errs.push(`console: ${m.text().slice(0, 300)}`); });
    page.on("dialog", async (d) => { errs.push(`dialog ${d.type()}: ${d.message().slice(0, 200)}`); await d.dismiss().catch(() => {}); });
    const fr = { name: flow.name || `flow ${fi + 1}`, passed: true, steps: [], page_opened: true,
      url: new URL(flow.url || start.href, start).href };
    progress("flow", { name: fr.name, steps: (flow.steps || []).length });
    try {
      await page.goto(new URL(flow.url || start.href, start).href, { waitUntil: "load" });
    } catch (e) {
      fr.passed = false;
      fr.steps.push({ i: 0, do: "open", result: "fail", error: String(e.message).split("\n")[0] });
    }
    for (const [i, step] of (flow.steps || []).entries()) {
      if (!fr.passed && !flow.continue_on_error) {
        fr.steps.push({ i: i + 1, do: step.do, target: step.target, result: "skipped" });
        continue;
      }
      const before = errs.length;
      const t0 = Date.now();
      const rec = { i: i + 1, do: step.do, target: step.target, result: "ok" };
      try {
        const shot = await runStep(page, step, start, outDir, `flow${fi + 1}-step${i + 1}`);
        if (shot) rec.screenshot = shot;
        await page.waitForLoadState("networkidle", { timeout: 1500 }).catch(() => {});
      } catch (e) {
        rec.result = "fail";
        rec.error = String(e.message).split("\n")[0].slice(0, 300);
      }
      const newErrs = errs.slice(before);
      if (newErrs.length && rec.result === "ok") {
        rec.result = "fail";
        rec.error = newErrs.join(" | ");
      } else if (newErrs.length) {
        rec.errors = newErrs;
      }
      rec.ms = Date.now() - t0;
      if (rec.result === "fail") {
        fr.passed = false;
        const p = path.join(outDir, `flow${fi + 1}-fail${i + 1}.png`);
        await page.screenshot({ path: p, fullPage: true }).catch(() => {});
        rec.screenshot = p;
      }
      fr.steps.push(rec);
      progress("flow_step", { flow: fr.name, i: i + 1, do: step.do, result: rec.result });
    }
    results.push(fr);
    await page.close().catch(() => {});
  }
  r.flows = results;
}

let emitted = false;
function emit(obj) {
  if (emitted) return;
  emitted = true;
  process.stdout.write(JSON.stringify(obj));
}

async function main() {
  let input;
  try {
    input = JSON.parse(await readStdin());
  } catch (e) {
    emit({ ok: false, error: `bad input JSON: ${e.message}` });
    return;
  }
  try {
    emit(input.mode === "screenshot" ? await screenshot(input) : await uiTest(input));
  } catch (e) {
    emit({ ok: false, error: String(e.message).split("\n")[0].slice(0, 500) });
  }
}

// Imported by ux_web.mjs for its helpers: run only when executed directly.
const same = (a, b) => path.resolve(a).toLowerCase() === path.resolve(b).toLowerCase();
if (process.argv[1] && same(fileURLToPath(import.meta.url), process.argv[1])) {
  // A throw inside a Playwright event handler would otherwise kill the process with no output.
  process.on("uncaughtException", (e) => {
    emit({ ok: false, error: `harness crashed: ${String(e && e.message).slice(0, 300)}` });
    process.exit(1);
  });
  main();
}
