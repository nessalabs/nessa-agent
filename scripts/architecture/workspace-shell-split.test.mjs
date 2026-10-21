/** Bare Node, no `node_modules`, like the rule it tests. */
import assert from "node:assert/strict"
import test from "node:test"

import { importedPaths } from "./imported-paths.mjs"
import { workspaceShellSplitViolations } from "./workspace-shell-split.mjs"

const refused = (path, text) => workspaceShellSplitViolations(path, importedPaths(text))
const quoted = (target) => `"${target}"`

test("a layout does not import the store hooks or the command list", () => {
  const layout = "src/desktop/workspace/ui/layouts/workspace-layout.tsx"
  for (const target of [
    "../../adapters/store/hooks",
    "../../adapters/store/commands",
    "../../adapters/store/pane-commands",
  ]) {
    const violations = refused(layout, `import { x } from ${quoted(target)}`)
    assert.equal(violations.length, target.endsWith("pane-commands") ? 2 : 1, target)
  }
  assert.deepEqual(
    refused(layout, `import { IconButton } from ${quoted("../../../ui/icon-button")}`),
    [],
  )
})

test("Settings' layout is held to the same rule", () => {
  const layout = "src/desktop/settings/ui/settings-layout.tsx"
  assert.equal(
    refused(
      layout,
      `import { useWorkspaceDispatch } from ${quoted("../../workspace/adapters/store/hooks")}`,
    ).length,
    1,
  )
})

test("command implementations are imported only by the command list", () => {
  const list = "src/desktop/workspace/adapters/store/commands.ts"
  assert.deepEqual(
    refused(list, `export { loadWorkspace } from ${quoted("./index-commands")}`),
    [],
  )
  const caller = "src/desktop/workspace/ui/panes/pane.tsx"
  const violations = refused(
    caller,
    `import { sendMessage } from ${quoted("../../adapters/store/session-commands")}`,
  )
  assert.equal(violations.length, 1)
  assert.match(violations[0], /commands\.ts|command list/)
})

test("a view may select what it shows", () => {
  const view = "src/desktop/workspace/ui/layouts/switcher-host.tsx"
  assert.deepEqual(
    refused(
      view,
      `import { useWorkspaceSelector } from ${quoted("../../adapters/store/hooks")}`,
    ),
    [],
  )
})
