import assert from "node:assert/strict"
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"

import {
  nativeSmokeEvidenceLimits,
  renderNativeSmokeError,
  retainNativeSmokeFailure,
} from "./native-smoke-evidence.mjs"

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

test("nested primary, cleanup, and artifact failures retain their causes", () => {
  const timeout = new DOMException("automation handshake timed out", "TimeoutError")
  const primary = new Error("create WebDriver session failed", { cause: timeout })
  const cleanup = new Error("driver process group cleanup could not be verified")
  const artifact = new Error("failure artifact write was refused")
  const combined = new AggregateError([
    new AggregateError([primary, cleanup], "request and cleanup failed"),
    artifact,
  ])
  cleanup.cause = combined

  const rendered = renderNativeSmokeError(combined)

  assert.match(rendered, /create WebDriver session failed/)
  assert.match(rendered, /automation handshake timed out/)
  assert.match(rendered, /driver process group cleanup could not be verified/)
  assert.match(rendered, /failure artifact write was refused/)
  assert.match(rendered, /\[cycle\]/)
  assert.ok(Buffer.byteLength(rendered) <= nativeSmokeEvidenceLimits.errorBytes)
})

test("recursive rendering stops traversing after its item limit", () => {
  const failures = Array.from(
    { length: nativeSmokeEvidenceLimits.errorItems * 10 },
    (_, index) => new Error(`cleanup ${index}`),
  )
  const rendered = renderNativeSmokeError(new AggregateError(failures))

  assert.match(rendered, /cleanup 0/)
  assert.doesNotMatch(rendered, /cleanup 100/)
  assert.equal(rendered.match(/additional errors omitted/g)?.length, 1)
})

test("oversized logs and error metadata stay within UTF-8 byte budgets", () => {
  const root = mkdtempSync(join(tmpdir(), "nessa-native-evidence-large-"))
  try {
    const retained = retainNativeSmokeFailure(root, "bounded-instance", {
      logs: `discarded-prefix-${"🐇".repeat(40_000)}-retained-tail`,
      metadata: {
        lifecyclePhase: "session".repeat(10_000),
        error: `failure-${'\\"🐇'.repeat(30_000)}`,
        instance: "native-smoke",
        executableArtifacts: {
          application: "/tmp/" + "app".repeat(20_000),
          gateway: "/tmp/gateway",
        },
      },
    })
    const logs = readFileSync(join(retained, "harness.log"))
    const metadata = readFileSync(join(retained, "metadata.json"))

    assert.ok(logs.byteLength <= nativeSmokeEvidenceLimits.logsBytes)
    assert.ok(metadata.byteLength <= nativeSmokeEvidenceLimits.metadataBytes)
    assert.match(logs.toString("utf8"), /retained-tail$/)
    assert.doesNotThrow(() => JSON.parse(metadata.toString("utf8")))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
