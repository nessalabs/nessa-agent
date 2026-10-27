/** Stored inputs from another surface retain identity and render after dispatch. */
import assert from "node:assert/strict"
import { createHash, randomUUID } from "node:crypto"
import { readFileSync } from "node:fs"
import { join } from "node:path"

export async function uploadedInputFiles(run, step, { prefix, settled, shown }) {
  for (const [name, mimeType, header] of [
    ["report.pdf", "application/pdf", "%PDF-1.7\n"],
    ["archive.zip", "application/zip", "PK\x03\x04"],
    ["notes.txt", "text/plain", "phone file sending\n"],
  ]) {
    const bytes = Buffer.from(header + "0123456789abcdef".repeat(4500))
    const digest = `sha256:${createHash("sha256").update(bytes).digest("hex")}`
    const reference = { digest, mimeType, size: bytes.length, name }
    const begun = await run.client.attachments.begin(run.main.id, reference)
    const stored =
      begun.state === "stored"
        ? begun.stored
        : await run.client.attachments.upload(begun.ticket, {
            mimeType,
            bytes: new Blob([bytes], { type: mimeType }),
          })
    assert.deepEqual(stored, { digest, mimeType, size: bytes.length })
    const text = `${prefix} uploaded input ${step.index} ${name} ${randomUUID()}`
    const input = run.client.conversation.prepare(run.main.id, text, [], [], {
      uploads: [reference],
    })
    await run.client.conversation.submit(input)
    const { view, turn } = await settled(text)
    assert.deepEqual(turn.uploads, [reference])
    assert.deepEqual(
      readFileSync(join(view.runtime.workspace, `nessa-${digest.slice(7)}-${name}`)),
      bytes,
    )
    // The saved identity settles a second submission without a second turn.
    await run.client.conversation.submit(input)
    await shown(name)
  }
}
