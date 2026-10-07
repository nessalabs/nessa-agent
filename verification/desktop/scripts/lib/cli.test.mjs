import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { createRequire } from "node:module"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import { report } from "./cli.mjs"

const require = createRequire(import.meta.url)

test("a check's --out JSON is prettier-clean", () => {
  const dir = mkdtempSync(join(tmpdir(), "nessa-json-"))
  const path = join(dir, "perf-budget.json")
  try {
    const rep = report("perf-budget", { out: path })
    rep.add({
      name: "drag-drop",
      failures: ["longest frame 66.69999999999982 ms > 50 ms (runs: 67)"],
    })
    rep.finish()
    const bin = require.resolve("prettier/bin/prettier.cjs")
    const result = spawnSync(process.execPath, [bin, "--check", path], {
      encoding: "utf8",
    })
    assert.equal(result.status, 0, result.stderr)
    const document = JSON.parse(readFileSync(path, "utf8"))
    assert.equal(
      document.results[0].failures[0],
      "longest frame 66.69999999999982 ms > 50 ms (runs: 67)",
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
