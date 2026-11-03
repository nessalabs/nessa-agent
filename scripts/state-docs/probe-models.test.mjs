import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { fileURLToPath } from "node:url"

const script = fileURLToPath(new URL("./probe-models.mjs", import.meta.url))
test("probe command fails clearly on bare Node when its renderer is absent", () => {
  for (const [args, error] of [
    [[], "renderer-required"],
    [["--install"], "invalid-arguments"],
  ]) {
    const result = spawnSync(process.execPath, [script, ...args], {
      env: { ...process.env, NESSA_DOCS_ROOT: "" },
      encoding: "utf8",
      timeout: 5000,
    })
    assert.ifError(result.error)
    assert.equal(result.status, 2)
    assert.equal(result.stderr, "")
    assert.equal(JSON.parse(result.stdout).error, error)
  }
})
