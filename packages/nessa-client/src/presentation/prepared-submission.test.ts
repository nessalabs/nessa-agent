import { expect, it, vi } from "vitest"
import {
  createConversationApi,
  prepareSubmission,
  type PreparedSubmission,
} from "./conversation-api.js"
import {
  executionTargets,
  type ExecutionTarget,
} from "../application/execution-target.js"
import { NessaConversationMutationError } from "../application/conversation-mutation-error.js"

const first = "00000000-0000-4000-8000-000000000001"
const second = "00000000-0000-4000-8000-000000000002"
const intent = {
  conversationId: first,
  executionId: "same-turn",
  requestId: "action",
  mode: "steering" as const,
  text: "original",
}

it("captures message identities and copied references before any request (PS1)", async () => {
  const request = vi.fn(async () => ({ executionId: "same-turn", disposition: "queued" }))
  const api = createConversationApi({ request }, () => "generated")
  const image = {
    digest: `sha256:${"a".repeat(64)}`,
    mimeType: "image/png" as const,
    size: 1,
  }
  const file = { path: "/tmp/original" }
  const images = [image],
    files = [file]
  const input = { ...intent, attachments: images, files }
  const prepared = prepareSubmission(input)
  expect(request).not.toHaveBeenCalled()
  input.conversationId = second
  input.text = "edited"
  image.size = 2
  file.path = "/tmp/edited"
  images.length = 0
  files.length = 0
  expect(Object.isFrozen(prepared)).toBe(true)
  expect(Object.isFrozen(prepared.target)).toBe(true)
  expect(Object.isFrozen(prepared.attachments[0])).toBe(true)
  expect(Object.isFrozen(prepared.files[0])).toBe(true)
  const receipt = await api.submit(prepared)
  expect(receipt.conversationId).toBe(first)
  expect(receipt.target).toBe(prepared.target)
  expect(request).toHaveBeenCalledExactlyOnceWith("conversation.steer", {
    conversationId: first,
    executionId: "same-turn",
    requestId: "action",
    text: "original",
    attachments: [{ digest: `sha256:${"a".repeat(64)}`, mimeType: "image/png", size: 1 }],
    files: [{ path: "/tmp/original" }],
  })
})

it("fills optional identities once and preserves queued and steering modes (PS2)", async () => {
  let next = 0
  const request = vi.fn(async (_method: string, fields: Record<string, unknown>) => ({
    executionId: fields.executionId,
    disposition: "queued",
  }))
  const api = createConversationApi({ request }, () => `generated-${next++}`)
  const prepared = api.prepare(first, "message")
  expect(prepared).toMatchObject({
    executionId: "generated-0",
    requestId: "generated-1",
    mode: "queued",
  })
  await api.submit(prepared)
  await api.submit(prepared)
  expect(next).toBe(2)
  expect(request.mock.calls[1]).toEqual(request.mock.calls[0])
})

it("rejects forged or copied prepared values and targets before effects (PS3)", () => {
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "id")
  const prepared = prepareSubmission(intent)
  for (const copy of [
    { ...prepared },
    { ...prepared, conversationId: second },
    JSON.parse(JSON.stringify(prepared)),
  ])
    expect(() => api.submit(copy)).toThrow(TypeError)
  for (const copy of [
    { ...prepared.target },
    { ...prepared.target, conversationId: second },
    JSON.parse(JSON.stringify(prepared.target)),
  ]) {
    expect(() => api.stop(copy)).toThrow(TypeError)
    expect(() => api.remove(copy)).toThrow(TypeError)
  }
  expect(request).not.toHaveBeenCalled()
})

it("captures same execution names from each whole view without mixing conversations (PS4)", async () => {
  const view = {
    conversationId: first,
    messages: [{ executionId: "same-turn" }],
    pending: [{ executionId: "queued" }],
  }
  const targets = executionTargets(view)
  const other = executionTargets({ ...view, conversationId: second })
  view.conversationId = second
  view.messages[0]!.executionId = "replacement"
  const request = vi.fn(async (_method: string, fields: Record<string, unknown>) => ({
    requestId: fields.requestId,
    applied: true,
  }))
  const api = createConversationApi({ request }, () => "action")
  await api.remove(targets[1]!)
  await api.remove(other[0]!)
  expect(request.mock.calls.map((call) => call[1])).toEqual([
    { conversationId: first, executionId: "queued", requestId: "action" },
    { conversationId: second, executionId: "same-turn", requestId: "action" },
  ])
  expect(targets[0]).toMatchObject({ conversationId: first, executionId: "same-turn" })
})

it("rejects mismatched receipt then retries the original intent, ignoring public error fields (PS5)", async () => {
  const request = vi
    .fn()
    .mockResolvedValueOnce({ executionId: "other", disposition: "queued" })
    .mockResolvedValue({ executionId: "same-turn", disposition: "queued" })
  const api = createConversationApi({ request }, () => "id")
  const prepared = prepareSubmission(intent)
  const error = await api.submit(prepared).catch((cause: unknown) => cause)
  expect(error).toBeInstanceOf(NessaConversationMutationError)
  const failed = error as NessaConversationMutationError<
    Awaited<ReturnType<typeof api.submit>>
  >
  Reflect.set(failed, "conversationId", second)
  Reflect.set(failed, "executionId", "forged")
  const receipt = await failed.retry()
  expect(receipt.target).toBe(prepared.target)
  expect(receipt.conversationId).toBe(first)
  expect(request.mock.calls[1]).toEqual(request.mock.calls[0])
})

// Compiled by normal client:typecheck; these declarations never execute effects.
function staticContract(
  api: ReturnType<typeof createConversationApi>,
  prepared: PreparedSubmission,
) {
  // @ts-expect-error independent fields cannot manufacture a prepared value
  const raw: PreparedSubmission = {
    ...intent,
    attachments: [],
    files: [],
    target: prepared.target,
  }
  // @ts-expect-error independent IDs cannot manufacture a control target
  const target: ExecutionTarget = { conversationId: first, executionId: "same-turn" }
  // @ts-expect-error the independent-ID stop API is gone
  api.stop(first, "same-turn")
  // @ts-expect-error the independent-ID remove API is gone
  api.remove(first, "same-turn")
  // @ts-expect-error prepare is the only path into submit
  api.submit(first, "message")
  // @ts-expect-error no compatibility sending API remains
  api.send(first, "message")
  // @ts-expect-error captured image metadata is immutable as well as its list
  prepared.attachments[0]!.size = 2
  // @ts-expect-error captured file paths cannot be edited independently
  prepared.files[0]!.path = "/changed"
  // @ts-expect-error message identity cannot be edited independently
  prepared.conversationId = second
  return [raw, target]
}
void staticContract

it("validates copied image references when source getters change (PS14)", () => {
  let reads = 0
  const image = {
    digest: `sha256:${"a".repeat(64)}`,
    mimeType: "image/png" as const,
    get size() {
      return ++reads <= 2 ? 1 : 99_999_999
    },
  }
  expect(
    prepareSubmission({ ...intent, attachments: [image] }).attachments[0]!.size,
  ).toBe(1)
  expect(reads).toBe(1)
})
it("validates copied file paths when source getters change (PS14)", () => {
  let paths = 0
  const file = {
    get path() {
      return ++paths <= 3 ? "/tmp/valid" : "relative"
    },
  }
  expect(prepareSubmission({ ...intent, files: [file] }).files[0]!.path).toBe(
    "/tmp/valid",
  )
  expect(paths).toBe(1)
})

it("captures enclosing conversation before reading its child execution identities (PS18)", () => {
  const view = {
    conversationId: first,
    messages: [
      {
        get executionId() {
          view.conversationId = second
          return "captured"
        },
      },
    ],
    pending: [],
  }
  const captured = executionTargets(view)
  expect(captured).toEqual([{ conversationId: first, executionId: "captured" }])
})

it("captures owned arrays without calling caller slice, map, species or iterator (PS19)", async () => {
  const image = {
    digest: `sha256:${"a".repeat(64)}`,
    mimeType: "image/png" as const,
    size: 1,
  }
  const file = { path: "/original" }
  const images = [image],
    files = [file]
  for (const list of [images, files]) {
    Object.defineProperty(list, "slice", {
      value: () => {
        throw new Error("caller slice")
      },
    })
    Object.defineProperty(list, "map", {
      value: () => {
        throw new Error("caller map")
      },
    })
    Object.defineProperty(list, Symbol.iterator, {
      value: () => {
        throw new Error("caller iterator")
      },
    })
    Object.defineProperty(list, "constructor", {
      value: {
        get [Symbol.species]() {
          throw new Error("caller species")
        },
      },
    })
  }
  const prepared = prepareSubmission({ ...intent, attachments: images, files })
  expect(prepared.attachments).not.toBe(images)
  expect(prepared.files).not.toBe(files)
  image.size = 2
  file.path = "/edited"
  const request = vi.fn(async (_method: string, _fields: unknown) => ({
    executionId: intent.executionId,
    disposition: "queued",
  }))
  const api = createConversationApi({ request }, () => "id")
  await api.submit(prepared)
  expect(request.mock.calls[0]![1]).toMatchObject({
    attachments: [{ size: 1 }],
    files: [{ path: "/original" }],
  })
})

it("rejects oversized source lists before reading their entries and accepts the published bound (PS19)", () => {
  const images = new Array(11)
  Object.defineProperty(images, 0, {
    get() {
      throw new Error("entry read before capacity")
    },
  })
  expect(() => prepareSubmission({ ...intent, attachments: images })).toThrow(TypeError)
  const image = {
    digest: `sha256:${"a".repeat(64)}`,
    mimeType: "image/png" as const,
    size: 1,
  }
  expect(
    prepareSubmission({ ...intent, attachments: Array.from({ length: 10 }, () => image) })
      .attachments,
  ).toHaveLength(10)
})

it("publishes one validated receipt snapshot without rereading adapter getters (PS20)", async () => {
  let identities = 0,
    dispositions = 0
  const request = vi.fn(async () => ({
    get executionId() {
      return ++identities === 1 ? intent.executionId : "other"
    },
    get disposition() {
      return ++dispositions === 1 ? "queued" : "invalid"
    },
  }))
  const api = createConversationApi({ request }, () => "id")
  const prepared = prepareSubmission(intent)
  const receipt = await api.submit(prepared)
  expect(receipt).toMatchObject({
    conversationId: first,
    executionId: intent.executionId,
    disposition: "queued",
  })
  expect(receipt.target).toBe(prepared.target)
  expect(identities).toBe(1)
  expect(dispositions).toBe(1)
})

it.each(["conversationId", "executionId", "requestId", "text"] as const)(
  "rejects caller-owned %s scalars before preparing or any effect (PS24)",
  (field) => {
    const coercion = vi.fn(() => (field === "conversationId" ? first : "original"))
    const caller = { trim: coercion, toString: coercion, toJSON: coercion }
    const value = caller as unknown as string
    const request = vi.fn()
    const api = createConversationApi({ request }, () => "generated")
    expect(() => prepareSubmission({ ...intent, [field]: value })).toThrow(TypeError)
    expect(() =>
      api.prepare(
        field === "conversationId" ? value : first,
        field === "text" ? value : "original",
        [],
        [],
        {
          executionId: field === "executionId" ? value : "execution",
          requestId: field === "requestId" ? value : "request",
        },
      ),
    ).toThrow(TypeError)
    expect(request).not.toHaveBeenCalled()
    expect(coercion).not.toHaveBeenCalled()
    expect(Object.isFrozen(caller)).toBe(false)
  },
)

it("rejects nonprimitive scalar create/read/control inputs before effects (PS24)", async () => {
  const coercion = vi.fn(() => first)
  const value = {
    trim: coercion,
    toString: coercion,
    toJSON: coercion,
  } as unknown as string
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "generated")
  expect(() => api.create({ conversationId: value })).toThrow(TypeError)
  expect(() => api.create({ conversationId: first, requestId: value })).toThrow(TypeError)
  await expect(api.read(value)).rejects.toThrow(TypeError)
  expect(() => api.close(value)).toThrow(TypeError)
  expect(() => api.close(first, { requestId: value })).toThrow(TypeError)
  expect(request).not.toHaveBeenCalled()
  expect(coercion).not.toHaveBeenCalled()
})

it.each(["conversationId", "executionId"] as const)(
  "rejects a nonprimitive %s in a trusted target projection (PS24)",
  (field) => {
    const value = { toString: () => first, toJSON: () => second } as unknown as string
    expect(() =>
      executionTargets({
        conversationId: field === "conversationId" ? value : first,
        messages: [{ executionId: field === "executionId" ? value : "execution" }],
        pending: [],
      }),
    ).toThrow(TypeError)
  },
)
