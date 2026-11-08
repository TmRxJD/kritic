import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const pageHelpers = fs.readFileSync(
  new URL("../../src/native/web_page.js", import.meta.url),
  "utf8",
);

function nativePage() {
  const button = {
    tagName: "BUTTON",
    type: "button",
    innerText: "Cards",
    value: "",
    attributes: new Map(),
    getBoundingClientRect: () => ({ width: 80, height: 24 }),
    getAttribute(name) { return this.attributes.get(name) ?? null; },
    setAttribute(name, value) { this.attributes.set(name, value); },
    removeAttribute(name) { this.attributes.delete(name); },
  };
  const document = {
    body: { querySelectorAll: () => [button] },
    querySelector(selector) {
      if (selector.includes(":has-text(")) {
        const error = new Error("invalid selector");
        error.name = "SyntaxError";
        throw error;
      }
      return selector === ".domain-list button" ? button : null;
    },
    querySelectorAll(selector) {
      if (selector.startsWith("[role=button]")) return [button];
      return [];
    },
  };
  const window = {};
  vm.runInNewContext(pageHelpers, {
    window,
    document,
    getComputedStyle: () => ({ visibility: "visible", display: "block" }),
    XPathResult: { FIRST_ORDERED_NODE_TYPE: 9 },
  });
  return window.__kritic;
}

test("native flow selectors support CSS, text, and role selectors", () => {
  const kritic = nativePage();

  assert.equal(kritic.mark(".domain-list button").tag, "button");
  assert.equal(kritic.mark("text=Cards").tag, "button");
  assert.equal(kritic.mark("role=button[name='Cards']").tag, "button");
});

test("native flow gives actionable guidance for Playwright-only selectors", () => {
  const kritic = nativePage();

  assert.throws(
    () => kritic.mark(".domain-list button:has-text('Cards')"),
    /Invalid or unsupported native UI selector .*Native flows accept CSS, text=, role=, and xpath= selectors; Playwright-only extensions such as :has-text\(\) are not supported\./,
  );
});
