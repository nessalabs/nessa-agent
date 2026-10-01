/**
 * ADR 326: the desktop window's verticals depend in one direction —
 *
 *   widgets ← workspace ← subagents ← experiments
 *
 * — each importing only those before it: the workspace imports widgets' hosts
 * and no plugin, subagents and experiments build on widgets' contract and the
 * workspace, and widgets imports none of them. A vertical is
 * `src/desktop/<name>/`; what lies outside the four (the window's composition,
 * its shared parts, split panes, Settings) is not this rule's business.
 *
 * Every form of import `imported-paths.mjs` reads is held: named, type and
 * re-export, side-effect, `import()`, and a test's `vi.mock`, `vi.doMock`,
 * `vi.importActual` and `vi.importMock` — the forms `split-panes-boundary.mjs`
 * reads. Not `require()`, path aliases or specifiers built at runtime, which
 * the source does not use. Pure text: `check-architecture.mjs` runs on bare
 * Node.
 */
import { posix } from "node:path"

/** The verticals, in the order they may depend: each on those before it. */
export const desktopVerticals = ["widgets", "workspace", "subagents", "experiments"]

const desktop = "src/desktop"

/** The vertical a repository path lies in, or `null` outside the four. */
function verticalOf(path) {
  if (!path.startsWith(`${desktop}/`)) return null
  const name = path.slice(desktop.length + 1).split("/")[0]
  return desktopVerticals.includes(name) ? name : null
}

/** Where a relative specifier in `path` points, from the repository root. */
function resolved(path, specifier) {
  if (!specifier.startsWith(".")) return null
  return posix.normalize(posix.join(posix.dirname(path), specifier))
}

export function desktopVerticalViolations(path, specifiers) {
  const from = verticalOf(path)
  if (from === null) return []
  const violations = []
  for (const specifier of specifiers) {
    const target = resolved(path, specifier)
    const to = target === null ? null : verticalOf(target)
    if (to === null || to === from) continue
    if (desktopVerticals.indexOf(to) > desktopVerticals.indexOf(from))
      violations.push(
        `${from} imports no ${to}: the desktop's verticals depend widgets ← workspace ← subagents ← experiments, not ${specifier} (ADR 326)`,
      )
  }
  return violations
}
