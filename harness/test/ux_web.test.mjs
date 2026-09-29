// node --test harness/test/  (needs PLAYWRIGHT_BROWSERS_PATH pointing at the installed Chromium)
// The ux_critic web adapter: the observe / act / reset contract over JSON lines.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { spawn } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const fixture = (f) => pathToFileURL(path.join(here, "fixtures", "ux", f)).href;
let proc;
let lines;

async function call(req) {
  proc.stdin.write(JSON.stringify(req) + "\n");
  const { value } = await lines.next();
  return JSON.parse(value);
}

before(() => {
  proc = spawn(process.execPath, [path.join(here, "..", "ux_web.mjs")], { stdio: ["pipe", "pipe", "inherit"] });
  lines = readline.createInterface({ input: proc.stdout })[Symbol.asyncIterator]();
});
after(async () => {
  const r = await call({ op: "close" });
  assert.equal(r.closed, true);
  await new Promise((done) => proc.once("exit", done));
});

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "kritic-ux-"));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));

test("observe reports what a user perceives, and ids are stable across a reset", async () => {
  assert.equal((await call({ op: "start", url: fixture("labels-bad.html"), ignore: ["^Settings$"] })).ok, true);
  assert.equal((await call({ op: "reset" })).ok, true);
  const a = (await call({ op: "observe", marks: path.join(tmp, "m.png"), axe: true })).obs;
  const buttons = a.controls.filter((c) => c.role === "button");
  assert.equal(buttons.length, 9);
  assert.ok(buttons.every((c) => c.label === "" && c.text === "" && c.tooltip === "" && c.enabled), JSON.stringify(buttons[0]));
  assert.ok(buttons.every((c) => c.rect[2] >= 24 && c.rect[3] >= 24));
  assert.ok(fs.existsSync(a.marked));
  assert.ok(Array.isArray(a.axe));
  await call({ op: "reset" });
  const b = (await call({ op: "observe" })).obs;
  assert.deepEqual(b.controls.map((c) => c.id), a.controls.map((c) => c.id));
  assert.equal(b.fingerprint, a.fingerprint);
});

test("act changes the screen; unknown ids are errors, not no-ops", async () => {
  await call({ op: "start", url: fixture("modes-bad.html") });
  await call({ op: "reset" });
  const home = (await call({ op: "observe" })).obs;
  assert.equal((await call({ op: "act", id: "tab:Editor#1" })).ok, true);
  const editor = (await call({ op: "observe" })).obs;
  assert.notEqual(editor.fingerprint, home.fingerprint);
  assert.match(editor.text, /Map editor/);
  const bad = await call({ op: "act", id: "button:Nope#1" });
  assert.equal(bad.ok, false);
  assert.match(bad.error, /no control/);
});

test("disabled controls are reported with their reason; ignored and destructive ones are skipped", async () => {
  await call({ op: "start", url: fixture("group-good.html"), ignore: ["^Theme$"] });
  await call({ op: "reset" });
  const o = (await call({ op: "observe" })).obs;
  const apply = o.controls.find((c) => c.label === "Apply trim");
  assert.equal(apply.enabled, false);
  assert.match(apply.tooltip, /in and out points/);
  assert.equal(o.controls.find((c) => c.label === "Theme").skip, "ignored");
});
