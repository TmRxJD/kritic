import { realpathSync, statSync } from "node:fs";
import { isAbsolute, relative, resolve } from "node:path";

const within = (root, candidate) => {
  const rel = relative(root, candidate);
  return rel !== ".." && !rel.startsWith("..\\") && !rel.startsWith("../") && !isAbsolute(rel);
};

// Fixture HTTP requests must not read sibling files, alternate streams or escaping symlinks.
export function fixturePath(requestUrl, roots) {
  let pathname;
  try { pathname = decodeURIComponent(new URL(requestUrl, "http://localhost").pathname); }
  catch { return null; }
  if (pathname.includes("\\") || pathname.includes(":") || pathname.includes("\0")) return null;
  for (const dir of roots) {
    try {
      const root = realpathSync(dir);
      const candidate = resolve(root, `.${pathname}`);
      if (!within(root, candidate)) continue;
      const actual = realpathSync(candidate);
      if (within(root, actual) && statSync(actual).isFile()) return actual;
    } catch { /* Missing fixture is a normal 404, never a fallback outside the roots. */ }
  }
  return null;
}
