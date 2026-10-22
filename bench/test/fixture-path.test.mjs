import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, realpathSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fixturePath } from "../fixture-path.mjs";

test("fixture paths preserve root priority and reject encoded traversal or invalid URLs", () => {
  const scratchRoot = realpathSync(tmpdir());
  const dir = mkdtempSync(join(scratchRoot, "kritic-fixture-path-"));
  try {
    const first = join(dir, "first"), second = join(dir, "second");
    mkdirSync(first); mkdirSync(second);
    writeFileSync(join(first, "page.html"), "first");
    writeFileSync(join(second, "page.html"), "second");
    writeFileSync(join(dir, "private.html"), "private");
    symlinkSync(dir, join(first, "outside"), "junction");
    assert.equal(fixturePath("/page.html", [first, second]), join(first, "page.html"));
    for (const url of ["/..%2fprivate.html", "/%2e%2e%5cprivate.html", "/outside/private.html", "/page.html:secret", "/%00", "/%ZZ", "/missing.html"]) {
      assert.equal(fixturePath(url, [first, second]), null, url);
    }
  } finally {
    const actual = realpathSync(dir);
    assert.equal(dirname(actual), scratchRoot, "cleanup must stay within the verified scratch root");
    rmSync(actual, { recursive: true, force: true });
  }
});
