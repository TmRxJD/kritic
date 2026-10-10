import test from "node:test";
import assert from "node:assert/strict";
import { compileIgnorePatterns } from "../ignore_patterns.mjs";

test("ignore regexes match labels and reject unsupported or excessive patterns", () => {
  const [pattern] = compileIgnorePatterns(["^Delete\\b"]);
  assert.equal(pattern.matcher("Delete account").find(), true);
  assert.equal(pattern.matcher("Undo Delete").find(), false);
  assert.throws(() => compileIgnorePatterns(["(a)\\1"]));
  assert.throws(() => compileIgnorePatterns(["a".repeat(1025)]));
  assert.throws(() => compileIgnorePatterns(Array(65).fill("a")));
});

test("nested quantifiers safely handle long nonmatching page labels", () => {
  const [pattern] = compileIgnorePatterns(["(a+)+$"]);
  assert.equal(pattern.matcher("a".repeat(100000) + "!").find(), false);
});
