import assert from "node:assert/strict"
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"

import { retainNativeSmokeFailure } from "./native-smoke-evidence.mjs"

test("failure retention writes only the selected bounded evidence", () => {
  const root = mkdtempSync(join(tmpdir(), "nessa-native-evidence-"))
  try {
    const metadata = {
      lifecyclePhase: "WebDriver session requested",
      sessionCreated: false,
      pids: { application: 1234 },
    }
    const retained = retainNativeSmokeFailure(root, "isolated-instance", {
      logs: "[driver] startup cause\n",
      metadata,
    })

    assert.deepEqual(readdirSync(retained).sort(), ["harness.log", "metadata.json"])
    assert.equal(
      readFileSync(join(retained, "harness.log"), "utf8"),
      "[driver] startup cause\n",
    )
    assert.deepEqual(
      JSON.parse(readFileSync(join(retained, "metadata.json"), "utf8")),
      metadata,
    )
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
