/** Bare Node, no `node_modules`, like the rule it tests. */
import assert from "node:assert/strict"
import test from "node:test"

import { wholeWorkspaceViolations } from "./whole-workspace.mjs"

const view = "src/desktop/workspace/ui/panes/pane.tsx"

test("a selector returning the whole workspace is refused, however it is written", () => {
  for (const text of [
    "useWorkspaceSelector((state) => state.workspace)",
    "useWorkspaceSelector((state: DesktopState) => state.workspace)",
    "useSelector(s => s.workspace)",
    "useSelector(({ workspace }) => workspace)",
    "export const selectWorkspace = (state: Root) => state.workspace\n",
    "const all = useWorkspaceSelector((state) => state.workspace, shallowEqual)",
  ])
    assert.equal(wholeWorkspaceViolations(view, text).length, 1, text)
})

test("reading a part of the workspace is not refused", () => {
  for (const text of [
    "useWorkspaceSelector((state) => state.workspace.panes)",
    "useWorkspaceSelector((state) => state.workspace.chrome.sidebarWidth)",
    "const panes = getState().workspace.panes",
    "(current, previous) => current.workspace.panes !== previous.workspace.panes",
    "useWorkspaceSelector(({ workspace }) => workspace.view)",
  ])
    assert.deepEqual(wholeWorkspaceViolations(view, text), [], text)
})

test("only the desktop window's product code is this rule's business", () => {
  const text = "useSelector((state) => state.workspace)"
  assert.deepEqual(wholeWorkspaceViolations("src/panel/ui/app.tsx", text), [])
  assert.deepEqual(
    wholeWorkspaceViolations("src/desktop/workspace/ui/x.test.tsx", text),
    [],
  )
})

test("the desktop window in this repository satisfies the rule", async () => {
  const { readdirSync, readFileSync, statSync } = await import("node:fs")
  const { dirname, join, relative } = await import("node:path")
  const { fileURLToPath } = await import("node:url")
  const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..")
  const walk = (dir) =>
    readdirSync(dir).flatMap((name) => {
      const path = join(dir, name)
      return statSync(path).isDirectory() ? walk(path) : [path]
    })
  for (const file of walk(join(root, "src", "desktop")).filter((f) =>
    /\.tsx?$/.test(f),
  )) {
    const path = relative(root, file).split("\\").join("/")
    assert.deepEqual(wholeWorkspaceViolations(path, readFileSync(file, "utf8")), [], path)
  }
})
