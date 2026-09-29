/** Bare Node, no `node_modules`, like the rule it tests. */
import assert from "node:assert/strict"
import test from "node:test"

import { importedPaths } from "./imported-paths.mjs"
import { splitPanesBoundaryViolations } from "./split-panes-boundary.mjs"

const host = "src/desktop/workspace/ui/panes/pane-grid.tsx"
const refused = (path, text) => splitPanesBoundaryViolations(path, importedPaths(text))

test("a host imports the barrel, a model file, or — from a test — the testing entry", () => {
  for (const text of [
    'import { SplitPanes } from "../../../split-panes"',
    'import { marks } from "../../../split-panes/index"',
    'import type { PaneRoom } from "../../../split-panes/model/pane-sizing"',
    'import { paneFrame } from "../../../split-panes/testing"',
    'const m = await import("../../../split-panes/model/drop")',
    'vi.mock("../../../split-panes", () => ({}))',
  ])
    assert.deepEqual(refused(host, text), [], text)
})

test("a host reaching past them is refused, however it imports", () => {
  for (const text of [
    'import { useSplitPanesDrag } from "../../../split-panes/adapters/dom/drag"',
    'import "../../../split-panes/ui/split-panes.css"',
    'const drag = await import("../../../split-panes/adapters/dom/drag")',
    'vi.mock("../../../split-panes/ui/split-panes", () => ({}))',
    'import type { SplitPanesSource } from "../../../split-panes/application/ports"',
    'import { x } from "../../../split-panes/model/nested/file"',
  ])
    assert.equal(refused(host, text).length, 1, text)
})

test("split panes import themselves, packages and the desktop's shared parts", () => {
  const own = "src/desktop/split-panes/ui/split-panes.tsx"
  for (const text of [
    'import { posix } from "node:path"',
    'import { marks } from "../adapters/dom/marks"',
    'import "./split-panes.css"',
    'import { ResizeEdge } from "../../ui/resize-edge"',
    'import { reducedMotion } from "../../adapters/motion-preference"',
  ])
    assert.deepEqual(refused(own, text), [], text)
})

test("split panes import no host", () => {
  const own = "src/desktop/split-panes/adapters/dom/drag.ts"
  for (const text of [
    'import { commitDrop } from "../../../workspace/adapters/store/commands"',
    'import { inMemorySource } from "../../../workspace"',
    'import type { DesktopStore } from "../../../store"',
    'import { SettingsHost } from "../../../settings"',
    'import { host } from "../../../../host"',
    'import { DesktopWindow } from "../../../ui/desktop-window"',
    'import { DesktopApp } from "../../../ui/desktop-app"',
    'import { Composer } from "../../../ui/composer"',
    'const real = await vi.importActual("../../../workspace")',
    'vi.mock("../../../workspace/adapters/store/commands")',
  ])
    assert.equal(refused(own, text).length, 1, text)
})

test("what is not a path into or out of split panes is not this rule's business", () => {
  assert.deepEqual(refused(host, 'import { x } from "../../adapters/store/commands"'), [])
  assert.deepEqual(
    refused("src/panel/app.tsx", 'import { y } from "./split-panes-like"'),
    [],
  )
})

test("a forbidden import after a regular expression that holds `/*` is still caught", () => {
  const path = "src/desktop/split-panes/adapters/dom/probe.ts"
  for (const source of [
    'const re = /* explanation */ /[/*]/; import("../../../workspace")',
    'if (ready) /[/*]/.test(x); import("../../../workspace")',
  ]) {
    const imports = importedPaths(source)
    assert.deepEqual(imports, ["../../../workspace"], source)
    assert.equal(splitPanesBoundaryViolations(path, imports).length, 1, source)
  }
})
