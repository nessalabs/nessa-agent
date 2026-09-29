/**
 * ADR 253: split panes are wrapped, not reached into, and know no host.
 *
 * - Outside `src/desktop/split-panes/`, a module imports the split panes'
 *   barrel (`split-panes`, `split-panes/index`) — what React renders, the
 *   drag, FLIP, the port a host implements, the names a host may use — or
 *   one of its pure model files (`split-panes/model/<file>`), which any
 *   module may import and a host's own model and use cases, which may not
 *   import React, must; and a test may import its test-only entry
 *   (`split-panes/testing`; `check-architecture.mjs` keeps every `testing`
 *   entry to tests). Nothing else of the module — an adapter, a component,
 *   a stylesheet — is anyone else's to import.
 * - Inside it, a module imports itself, packages, and the desktop window's
 *   shared parts (`src/desktop/adapters/`, `src/desktop/ui/`,
 *   `src/desktop/model/`) and nothing else: no host, the workspace included.
 *
 * Reading a stylesheet's text by URL in a test (`readFileSync(new URL(…))`)
 * is not an import and is not this rule's business: a sheet is read as the
 * published text it is. Pure text, like every rule here:
 * `check-architecture.mjs` runs on bare Node, and specifiers come from
 * `imported-paths.mjs`.
 */
import { posix } from "node:path"

const module = "src/desktop/split-panes"
const shared = ["src/desktop/adapters/", "src/desktop/ui/", "src/desktop/model/"]

/** Where a relative specifier in `path` points, from the repository root, without an extension. */
function resolved(path, specifier) {
  if (!specifier.startsWith(".")) return null
  return posix
    .normalize(posix.join(posix.dirname(path), specifier))
    .replace(/\.(?:tsx?|mjs|js)$/, "")
}

const inside = (target) => target === module || target.startsWith(`${module}/`)

/** Whether a module outside split panes may import `target`, a path inside it. */
function publishedEntry(target) {
  const rest = target.slice(module.length)
  return (
    rest === "" ||
    rest === "/index" ||
    rest === "/testing" ||
    /^\/model\/[^/]+$/.test(rest)
  )
}

export function splitPanesBoundaryViolations(path, specifiers) {
  const ownFile = inside(path.replace(/\.[^./]+$/, ""))
  const violations = []
  for (const specifier of specifiers) {
    const target = resolved(path, specifier)
    if (target === null) continue
    if (!ownFile && inside(target) && !publishedEntry(target))
      violations.push(
        `other modules import the split-panes barrel, its model or its testing entry, not ${specifier} (ADR 253)`,
      )
    if (ownFile && !inside(target) && !shared.some((part) => target.startsWith(part)))
      violations.push(
        `split panes import no host, only the desktop's shared parts: not ${specifier} (ADR 253)`,
      )
  }
  return violations
}
