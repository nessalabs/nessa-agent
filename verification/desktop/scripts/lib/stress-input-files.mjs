/** The production Swift phone sends into the loaded conversation. */
import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { accessSync, constants, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import {
  startProbeProcess,
  stopProbeProcess,
} from "../../../../scripts/subagent-contracts/processes.mjs"

export function phoneFilesVerifier() {
  const executable = process.env.NESSA_PHONE_FILES_VERIFIER
  assert.ok(
    executable,
    "inputFiles requires NESSA_PHONE_FILES_VERIFIER built from nessa-ios",
  )
  accessSync(executable, constants.X_OK)
  return executable
}

async function phoneFiles(run, step, prefix) {
  const configuration = join(run.stack.scratch, `phone-files-${step.index}.json`)
  writeFileSync(
    configuration,
    JSON.stringify({
      endpoint: new URL("/session", run.stack.endpoint).href,
      credential: run.stack.credential,
      conversationID: run.main.id,
      prefix: `${prefix} ${step.index}`,
    }),
    { mode: 0o600 },
  )
  let owned
  let timer
  let output = ""
  try {
    owned = startProbeProcess(phoneFilesVerifier(), ["--stress-files", configuration])
    const result = await new Promise((resolve) => {
      timer = setTimeout(() => resolve({ timeout: true }), 180_000)
      const keep = (chunk) => {
        output += chunk.toString()
        if (output.length > 1024 * 1024) resolve({ overflow: true })
      }
      owned.child.stdout.on("data", keep)
      owned.child.stderr.on("data", keep)
      owned.child.on("error", (error) => resolve({ error: error.message }))
      owned.child.on("close", (code, signal) => resolve({ code, signal }))
    })
    assert.equal(
      result.code,
      0,
      `Swift phone failed: ${JSON.stringify(result)}\n${output.slice(-8192)}`,
    )
    const line = output.split("\n").find((line) => line.startsWith("PHONE_FILES_RESULT:"))
    assert.ok(line, "Swift phone returned no sending evidence")
    return JSON.parse(line.slice("PHONE_FILES_RESULT:".length))
  } finally {
    clearTimeout(timer)
    try {
      if (owned) {
        const cleanup = await stopProbeProcess(owned)
        assert.equal(
          cleanup.kind,
          "stopped",
          "Swift phone process cleanup was not confirmed",
        )
      }
    } finally {
      rmSync(configuration, { force: true })
    }
  }
}

export async function uploadedInputFiles(run, step, { prefix, settled, shown }) {
  const sent = await phoneFiles(run, step, prefix)
  assert.equal(sent.length, 6)
  for (const usesOutbox of [false, true]) {
    for (const [name, mimeType, header] of [
      ["report.pdf", "application/pdf", "%PDF-1.7\n"],
      ["archive.zip", "application/zip", "PK\x03\x04"],
      ["notes.txt", "text/plain", "phone file sending\n"],
    ]) {
      const bytes = Buffer.from(header + "0123456789abcdef".repeat(4500))
      const digest = `sha256:${createHash("sha256").update(bytes).digest("hex")}`
      const reference = { digest, mimeType, size: bytes.length, name }
      const result = sent.find(
        (item) => item.name === name && item.usesOutbox === usesOutbox,
      )
      assert.ok(result, `${usesOutbox ? "outbox" : "journal"} did not send ${name}`)
      assert.deepEqual(
        {
          digest: result.digest,
          mimeType: result.mimeType,
          size: result.size,
          name: result.name,
        },
        reference,
      )
      const { view, turn } = await settled(result.text)
      assert.equal(turn.executionId, result.executionID)
      assert.equal(
        view.messages.filter((item) => item.executionId === result.executionID).length,
        1,
      )
      assert.deepEqual(turn.uploads, [reference])
      assert.deepEqual(
        readFileSync(join(view.runtime.workspace, `nessa-${digest.slice(7)}-${name}`)),
        bytes,
      )
      await shown(name)
    }
  }
}
