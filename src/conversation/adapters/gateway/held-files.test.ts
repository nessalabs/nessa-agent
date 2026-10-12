import { NessaHeldFileError, type AttachmentApi } from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import { ConversationUnavailableError, HeldFileReadError } from "../../application/ports"
import type { PublishedFile } from "../../model"
import { heldFileReader } from "./held-files"

const file = { digest: `sha256:${"a".repeat(64)}`, mimeType: "image/png", size: 3 }

function attachments(fetch: AttachmentApi["fetch"]): AttachmentApi {
  return { fetch } as AttachmentApi
}

describe("heldFileReader", () => {
  it("hands back what the client verified, asked for exactly the file named", async () => {
    const bytes = new Blob(["abc"], { type: "image/png" })
    const fetch = vi.fn(async () => bytes)
    const signal = new AbortController().signal
    const read = heldFileReader(() => attachments(fetch))
    // A published file's name and turn are for people; only what it is held as is asked.
    const published: PublishedFile = { ...file, name: "x.png", executionId: "e" }
    expect(await read("conversation", published, signal)).toBe(bytes)
    expect(fetch).toHaveBeenCalledExactlyOnceWith("conversation", file, { signal })
  })

  it.each([
    ["not_held", "not-held"],
    ["conversation_deleted", "deleted"],
    ["integrity", "damaged"],
    ["temporarily_unavailable", "unavailable"],
    ["unreachable", "unavailable"],
    ["unexpected_response", "unavailable"],
    ["aborted", "unavailable"],
  ] as const)("says the client's %s as %s", async (code, reason) => {
    const cause = new NessaHeldFileError(code)
    const read = heldFileReader(() =>
      attachments(async () => {
        throw cause
      }),
    )
    const error = await read("c", file, new AbortController().signal).catch(
      (error: unknown) => error,
    )
    expect(error).toBeInstanceOf(HeldFileReadError)
    expect(error).toMatchObject({ reason, cause })
  })

  it("says there is no client to ask as unavailable", async () => {
    const read = heldFileReader(() => {
      throw new ConversationUnavailableError()
    })
    await expect(read("c", file, new AbortController().signal)).rejects.toMatchObject({
      reason: "unavailable",
    })
  })
})

it("stops waiting for a client still connecting once the read is abandoned", async () => {
  const read = heldFileReader(() => new Promise<AttachmentApi>(() => {}))
  const reading = new AbortController()
  const result = read("c", file, reading.signal).catch((error: unknown) => error)
  reading.abort()
  expect(await result).toMatchObject({ reason: "unavailable" })
})
