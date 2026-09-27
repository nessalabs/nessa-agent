import { expect, it, vi } from "vitest"

import { attachmentUploadUrl } from "../application/attachment-upload.js"
import { fetchAttachmentUpload, type UploadFetch } from "./attachment-upload.js"

const ticket = "cd".repeat(32)
const bytes = new Blob(["raw"], { type: "image/heic" })
const stored = { digest: `sha256:${"ef".repeat(32)}`, mimeType: "image/jpeg", size: 2 }

function answering(status: number, body: string): UploadFetch {
  return vi.fn(async () => ({ status, text: async () => body }))
}

it("derives the upload route from the session URL: same host, http, no socket path", () => {
  expect(attachmentUploadUrl("ws://127.0.0.1:7420/session")).toBe(
    "http://127.0.0.1:7420/attachments",
  )
  expect(attachmentUploadUrl("wss://gateway.example/browser/session")).toBe(
    "https://gateway.example/attachments",
  )
  expect(attachmentUploadUrl("ws://[::1]:7421/session")).toBe(
    "http://[::1]:7421/attachments",
  )
  expect(() => attachmentUploadUrl("http://127.0.0.1:7420")).toThrow(TypeError)
})

it("puts the raw bytes with the ticket, no credentials, and no redirects", async () => {
  const fetch = answering(200, JSON.stringify(stored))
  const signal = new AbortController().signal
  const reply = await fetchAttachmentUpload("http://gateway/attachments", fetch).put({
    ticket,
    mimeType: "image/heic",
    bytes,
    signal,
  })
  expect(reply).toEqual({ status: 200, body: stored })
  expect(fetch).toHaveBeenCalledExactlyOnceWith("http://gateway/attachments", {
    method: "PUT",
    headers: { "x-nessa-upload-ticket": ticket, "content-type": "image/heic" },
    body: bytes,
    credentials: "omit",
    redirect: "error",
    signal,
  })
})

it.each([
  [401, '{"code":"ticket_invalid"}', { code: "ticket_invalid" }],
  [415, '{"code":"unsupported_image"}', { code: "unsupported_image" }],
  // Parsed, not judged: what a body means is decided by the caller of the port.
  [400, '{"code":7}', { code: 7 }],
  [400, "null", null],
  // Not small JSON: reported as an answer with no body, never guessed at.
  [502, "<html>Bad gateway</html>", undefined],
  [204, "", undefined],
  [400, `{"code":"${"x".repeat(2000)}"}`, undefined],
])("reads a %i answer %s", async (status, text, body) => {
  const transport = fetchAttachmentUpload(
    "http://gateway/attachments",
    answering(status, text),
  )
  expect(await transport.put({ ticket, mimeType: "image/heic", bytes })).toEqual({
    status,
    body,
  })
})

it("rejects only when no answer arrived, and still reports an answer whose body fails", async () => {
  const offline = fetchAttachmentUpload("http://gateway/attachments", () =>
    Promise.reject(new TypeError("Failed to fetch")),
  )
  await expect(offline.put({ ticket, mimeType: "image/heic", bytes })).rejects.toThrow(
    "Failed to fetch",
  )
  const truncated = fetchAttachmentUpload("http://gateway/attachments", async () => ({
    status: 503,
    text: () => Promise.reject(new Error("connection reset")),
  }))
  expect(await truncated.put({ ticket, mimeType: "image/heic", bytes })).toEqual({
    status: 503,
    body: undefined,
  })
})
