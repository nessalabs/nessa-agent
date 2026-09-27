import assert from "node:assert/strict"
import test from "node:test"

import { builtExecutables } from "./native-smoke-build.mjs"

test("launch paths come from the exact Cargo build artifacts", () => {
  const output = [
    { reason: "compiler-artifact", target: { name: "dependency" }, executable: null },
    {
      reason: "compiler-artifact",
      target: { name: "nessa-app" },
      executable: "/cache/native/x86_64-unknown-linux-gnu/debug/nessa-app",
    },
    {
      reason: "compiler-artifact",
      target: { name: "nessa" },
      executable: "/cache/native/x86_64-unknown-linux-gnu/debug/nessa",
    },
    { reason: "build-finished", success: true },
  ]
    .map(JSON.stringify)
    .join("\n")

  assert.deepEqual(Object.fromEntries(builtExecutables(output)), {
    "nessa-app": "/cache/native/x86_64-unknown-linux-gnu/debug/nessa-app",
    nessa: "/cache/native/x86_64-unknown-linux-gnu/debug/nessa",
  })
})
