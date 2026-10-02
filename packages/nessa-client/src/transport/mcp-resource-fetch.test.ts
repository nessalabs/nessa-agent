import { expect, it, vi } from "vitest"

import { mcpResourceUrl } from "../application/mcp-resource-fetch.js"
import { fetchMcpResource, type ResourceFetch } from "./mcp-resource-fetch.js"

const ticket = "Tk_-".repeat(10) + "abc"
const url = "http://gateway/mcp-resources"

/** A body that arrives in the given chunks, recording whether it was cancelled. */
function body(chunks: number[][]) {
  const state = { cancelled: false, pulled: 0 }
  const stream = new ReadableStream<Uint8Array>({
    pull(controller) {
      const next = chunks[state.pulled++]
      if (next === undefined) controller.close()
      else controller.enqueue(new Uint8Array(next))
    },
    cancel() {
      state.cancelled = true
    },
  })
  return { stream, state }
}

it("derives the resource route from the session URL: same host, http, no socket path", () => {
  expect(mcpResourceUrl("ws://127.0.0.1:7420/session")).toBe(
    "http://127.0.0.1:7420/mcp-resources",
  )
  expect(mcpResourceUrl("wss://gateway.example/browser/session")).toBe(
    "https://gateway.example/mcp-resources",
  )
  expect(() => mcpResourceUrl("http://127.0.0.1:7420")).toThrow(TypeError)
})

it("gets the bytes with the ticket in its header and never in the URL", async () => {
  const { stream } = body([[1, 2], [3]])
  const fetch = vi.fn<ResourceFetch>(async () => ({ status: 200, body: stream }))
  const signal = new AbortController().signal
  const reply = await fetchMcpResource(url, fetch).get({ ticket, maxBytes: 3, signal })
  expect(reply).toEqual({ status: 200, bytes: new Uint8Array([1, 2, 3]) })
  expect(fetch).toHaveBeenCalledExactlyOnceWith(url, {
    method: "GET",
    headers: { "x-nessa-resource-ticket": ticket },
    credentials: "omit",
    redirect: "error",
    cache: "no-store",
    signal,
  })
  const [target] = fetch.mock.calls[0]!
  expect(target).not.toContain(ticket)
  expect(new URL(target).search).toBe("")
})

it("stops reading one byte past what was described", async () => {
  const { stream, state } = body([[1, 2], [3, 4, 5], [6], [7]])
  const reply = await fetchMcpResource(url, async () => ({
    status: 200,
    body: stream,
  })).get({ ticket, maxBytes: 3 })
  // Enough to know it is too long, and no more held.
  expect(reply.bytes).toEqual(new Uint8Array([1, 2, 3, 4]))
  expect(state.cancelled).toBe(true)
  expect(state.pulled).toBeLessThan(4)
})

it("reads no body for a refusal, and lets the body go", async () => {
  const { stream, state } = body([[1]])
  const reply = await fetchMcpResource(url, async () => ({
    status: 404,
    body: stream,
  })).get({ ticket, maxBytes: 1 })
  expect(reply).toEqual({ status: 404, bytes: new Uint8Array() })
  expect(state.cancelled).toBe(true)
  const empty = await fetchMcpResource(url, async () => ({
    status: 200,
    body: null,
  })).get({
    ticket,
    maxBytes: 0,
  })
  expect(empty).toEqual({ status: 200, bytes: new Uint8Array() })
})

it("rejects when no answer, or no whole body, arrived", async () => {
  await expect(
    fetchMcpResource(url, () => Promise.reject(new TypeError("Failed to fetch"))).get({
      ticket,
      maxBytes: 1,
    }),
  ).rejects.toThrow("Failed to fetch")
  const broken = new ReadableStream<Uint8Array>({
    pull(controller) {
      controller.error(new Error("connection reset"))
    },
  })
  await expect(
    fetchMcpResource(url, async () => ({ status: 200, body: broken })).get({
      ticket,
      maxBytes: 1,
    }),
  ).rejects.toThrow("connection reset")
})
