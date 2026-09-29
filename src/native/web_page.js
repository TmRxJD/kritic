// In-page helpers for kritic's web tester, installed once per document as window.__kritic.
// Everything here observes; the only DOM writes are the data-kritic-id tags and the temporary
// set-of-marks overlay.
(() => {
  if (window.__kritic) return;
  const K = {};

  // Tag candidates with data-kritic-id and describe them. `all` (ux_critic) also returns disabled
  // controls and what a user perceives of each: accessible label, visible text, tooltip, on-screen
  // rect, whether it sits inline in text, and whether it shows as selected.
  K.collect = (opts = {}) => {
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
  };

  K.signature = () => {
    const s = document.body ? document.body.innerHTML : "";
    let h = 5381;
    for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
    let store = "";
    try {
      store = JSON.stringify(Object.entries(localStorage)) + JSON.stringify(Object.entries(sessionStorage));
    } catch { /* storage may be blocked */ }
    return { dom: h, len: s.length, store };
  };

  K.links = () => [...document.querySelectorAll("a[href]")].map((a) => a.href);
  K.storageKeys = () => { try { return Object.keys(localStorage); } catch { return []; } };
  K.text = () => (document.body ? document.body.innerText : "");
  K.status = () => {
    const nav = performance.getEntriesByType("navigation")[0];
    return nav && nav.responseStatus ? nav.responseStatus : null;
  };

  // ---- selectors for flows and persistence checks
  // css (default), "text=Save" (smallest visible element whose text contains it, case-insensitive),
  // "role=button[name='Save']" (role by ARIA or tag, accessible name contains it), "xpath=//...".
  const ROLE_TAGS = { button: "button,input[type=button],input[type=submit],input[type=reset],summary", link: "a[href]",
    textbox: "input:not([type]),input[type=text],input[type=email],input[type=search],input[type=url],input[type=tel],input[type=password],input[type=number],textarea",
    checkbox: "input[type=checkbox]", radio: "input[type=radio]", combobox: "select", heading: "h1,h2,h3,h4,h5,h6", tab: "", switch: "" };
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== "hidden" && s.display !== "none";
  };
  const accName = (el) => (el.getAttribute("aria-label") || (el.labels && el.labels[0] && el.labels[0].innerText) ||
    el.innerText || el.value || el.getAttribute("title") || el.getAttribute("placeholder") || "").trim();
  K.resolve = (sel) => {
    if (sel.startsWith("xpath=")) {
      return document.evaluate(sel.slice(6), document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;
    }
    if (sel.startsWith("text=")) {
      let want = sel.slice(5).trim();
      const exact = /^".*"$|^'.*'$/.test(want);
      if (exact) want = want.slice(1, -1);
      const lower = want.toLowerCase();
      let best = null;
      for (const el of document.body.querySelectorAll("*")) {
        if (!visible(el)) continue;
        const t = (el.innerText || el.value || "").trim();
        const hit = exact ? t === want : t.toLowerCase().includes(lower);
        if (hit && (!best || best.contains(el))) best = el;
      }
      return best;
    }
    const role = /^role=([a-z]+)(?:\[name=(['"])(.*)\2\])?$/i.exec(sel);
    if (role) {
      const [, r, , name] = role;
      const tags = ROLE_TAGS[r.toLowerCase()];
      const cands = [...document.querySelectorAll(`[role=${r}]${tags ? "," + tags : ""}`)].filter(visible);
      return cands.find((el) => name === undefined || accName(el).toLowerCase().includes(name.toLowerCase())) || null;
    }
    return document.querySelector(sel);
  };
  // Tag the resolved element so the driver can act on it; returns what it is.
  K.mark = (sel) => {
    const el = K.resolve(sel);
    if (!el) return null;
    for (const x of document.querySelectorAll("[data-kritic-target]")) x.removeAttribute("data-kritic-target");
    el.setAttribute("data-kritic-target", "1");
    return { tag: el.tagName.toLowerCase(), type: (el.type || "").toLowerCase(), visible: visible(el) };
  };

  // ---- element helpers, by [data-kritic-id] or [data-kritic-target]
  K.el = (key) => document.querySelector(key === "@target" ? "[data-kritic-target]" : `[data-kritic-id="${key}"]`);
  K.point = (key) => {
    const el = K.el(key);
    if (!el) return null;
    el.scrollIntoView({ block: "center", inline: "center" });
    const r = el.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, tag: el.tagName.toLowerCase(), type: (el.type || "").toLowerCase(),
      disabled: !!el.disabled };
  };
  // Fill like a user typing, through the native setter so framework-controlled inputs update.
  K.fill = (key, value) => {
    const el = K.el(key);
    if (!el) return false;
    el.focus();
    const proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : el.tagName === "SELECT" ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
    if (el.isContentEditable) el.textContent = value;
    else if (setter) setter.call(el, value);
    else el.value = value;
    el.dispatchEvent(new InputEvent("input", { bubbles: true, data: value, inputType: "insertText" }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  };
  K.select = (key, which) => {
    const el = K.el(key);
    if (!el || el.tagName !== "SELECT") return false;
    if (typeof which === "number") el.selectedIndex = Math.min(which, el.options.length - 1);
    else {
      const i = [...el.options].findIndex((o) => o.value === which || o.text === which);
      if (i < 0) throw new Error(`no option "${which}"`);
      el.selectedIndex = i;
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return el.value;
  };
  K.optionCount = (key) => K.el(key)?.options?.length ?? 0;
  K.value = (key) => {
    const el = K.el(key);
    if (!el) return null;
    if (el.type === "checkbox" || el.type === "radio") return el.checked;
    return el.value ?? el.textContent ?? "";
  };
  K.textOf = (key) => {
    const el = K.el(key);
    return el ? (el.innerText || el.textContent || el.value || "") : null;
  };
  K.isVisible = (key) => {
    const el = K.el(key);
    return !!el && visible(el);
  };

  // ---- set-of-marks overlay for the ux_critic novice
  K.drawMarks = (marks) => {
    const layer = document.createElement("div");
    layer.id = "__kritic_marks";
    layer.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647";
    for (const m of marks) {
      const box = document.createElement("div");
      box.style.cssText = `position:fixed;left:${m.x}px;top:${m.y}px;width:${m.w}px;height:${m.h}px;outline:2px solid #e00;box-sizing:border-box`;
      const tag = document.createElement("div");
      tag.textContent = String(m.n);
      tag.style.cssText = `position:fixed;left:${Math.max(0, m.x - 2)}px;top:${Math.max(0, m.y - 14)}px;background:#e00;color:#fff;font:bold 12px/14px sans-serif;padding:0 3px;border-radius:2px`;
      layer.append(box, tag);
    }
    document.body.append(layer);
    return true;
  };
  K.clearMarks = () => { document.getElementById("__kritic_marks")?.remove(); return true; };

  window.__kritic = K;
})();
