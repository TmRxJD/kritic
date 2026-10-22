import { RE2JS } from "re2js";

// Operator regexes still run against page-controlled labels. Use a linear-time engine,
// and bound compilation work; unsupported backreferences/lookaround fail explicitly.
export function compileIgnorePatterns(patterns) {
  if (!Array.isArray(patterns) || patterns.length > 64) throw new Error("ignore must contain at most 64 regex strings");
  return patterns.map((pattern) => {
    if (typeof pattern !== "string" || pattern.length > 1024) throw new Error("ignore regex must be a string of at most 1024 characters");
    return RE2JS.compile(pattern);
  });
}
