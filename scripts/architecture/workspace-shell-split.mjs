/**
 * Issue 280: the workspace layout and Settings' layout render from props, and
 * the index, pane, and session command implementations are reached through
 * `commands.ts`.
 *
 * Pure text, like every rule here: `check-architecture.mjs` runs on bare
 * Node, and specifiers come from `imported-paths.mjs`.
 */
import { posix } from "node:path"

/** The two modules that render and do not decide. */
const layouts = new Set([
  "src/desktop/workspace/ui/layouts/workspace-layout",
  "src/desktop/settings/ui/settings-layout",
])

const storeHooks = "src/desktop/workspace/adapters/store/hooks"
const commandList = "src/desktop/workspace/adapters/store/commands"
const commandImplementations = new Set([
  "src/desktop/workspace/adapters/store/index-commands",
  "src/desktop/workspace/adapters/store/pane-commands",
  "src/desktop/workspace/adapters/store/session-commands",
])

/** Where a relative specifier in `path` points, from the repository root, without an extension. */
function resolved(path, specifier) {
  if (!specifier.startsWith(".")) return null
  return posix
    .normalize(posix.join(posix.dirname(path), specifier))
    .replace(/\.(?:tsx?|mjs|js)$/, "")
}

export function workspaceShellSplitViolations(path, specifiers) {
  const file = path.replace(/\.[^./]+$/, "")
  const violations = []
  for (const specifier of specifiers) {
    const target = resolved(path, specifier)
    if (!target) continue
    if (
      layouts.has(file) &&
      (target === storeHooks ||
        target === commandList ||
        commandImplementations.has(target))
    ) {
      violations.push(`${file} renders from props and does not import ${target}`)
    }
    if (commandImplementations.has(target) && file !== commandList) {
      violations.push(
        `${target} is an implementation of the command list; import ${commandList}`,
      )
    }
  }
  return violations
}
