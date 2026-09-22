import assert from "node:assert/strict"
import test from "node:test"

import { runtimeExecutables } from "./runtime-layout.mjs"

test("desktop platform layouts name their bundled executables", () => {
  assert.deepEqual(runtimeExecutables("darwin"), {
    node: "node",
    gateway: "nessa",
    mcp: "nessa-mcp",
  })
  assert.equal(runtimeExecutables("linux"), runtimeExecutables("darwin"))
  assert.deepEqual(runtimeExecutables("win32"), {
    node: "node.exe",
    gateway: "nessa.exe",
    mcp: "nessa-mcp.exe",
  })
  assert.throws(() => runtimeExecutables("freebsd"), /does not support freebsd/)
})
