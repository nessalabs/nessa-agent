import { expect, it, vi } from "vitest"
import { permissionTargets, type PermissionTarget } from "../index.js"
import {
  createConversationApi,
  type ConversationApi,
} from "../presentation/conversation-api.js"

const first = "00000000-0000-4000-8000-000000000001"
const second = "00000000-0000-4000-8000-000000000002"
function source(conversationId = first) {
  return {
    conversationId,
    permissions: [
      {
        review: "a".repeat(32),
        executionId: "turn-a",
        permissionId: "review-a",
        options: [{ id: "allow-a" }],
      },
      {
        review: "b".repeat(32),
        executionId: "turn-b",
        permissionId: "review-b",
        options: [{ id: "deny-b" }],
      },
    ],
  }
}
function requester() {
  const request = vi.fn(async (_method: string, params: unknown) => ({
    requestId: (params as { requestId: string }).requestId,
    applied: true,
  }))
  return { request, api: createConversationApi({ request }, () => "action") }
}

it("PT1/PT7: answer and cancel keep each enclosing conversation and review together", async () => {
  const { api, request } = requester()
  const [a, b] = permissionTargets(source())
  const [other] = permissionTargets(source(second))
  await api.answer(a!, "allow-a")
  await api.cancel(b!, "withdrawn")
  await api.answer(other!, "allow-a")
  expect(request.mock.calls).toEqual([
    [
      "conversation.answer",
      {
        conversationId: first,
        executionId: "turn-a",
        review: "a".repeat(32),
        permissionId: "review-a",
        optionId: "allow-a",
        requestId: "action",
      },
    ],
    [
      "conversation.cancel",
      {
        conversationId: first,
        executionId: "turn-b",
        review: "b".repeat(32),
        permissionId: "review-b",
        reason: "withdrawn",
        requestId: "action",
      },
    ],
    [
      "conversation.answer",
      {
        conversationId: second,
        executionId: "turn-a",
        review: "a".repeat(32),
        permissionId: "review-a",
        optionId: "allow-a",
        requestId: "action",
      },
    ],
  ])
})

it("PT2: changing a source projection cannot replace a captured identity or choice", async () => {
  const view = source()
  const [target] = permissionTargets(view)
  view.conversationId = second
  Object.assign(view.permissions[0]!, { executionId: "turn-b", permissionId: "review-b" })
  view.permissions[0]!.options[0]!.id = "deny-b"
  const { api, request } = requester()
  expect(() => Object.assign(target!, { conversationId: second })).toThrow(TypeError)
  await api.answer(target!, "allow-a")
  expect(request.mock.calls[0]![1]).toMatchObject({
    conversationId: first,
    executionId: "turn-a",
    permissionId: "review-a",
    optionId: "allow-a",
  })
  expect(() => api.answer(target!, "deny-b")).toThrow(TypeError)
  expect(request).toHaveBeenCalledTimes(1)
})

it("PT3: valid identities from other targets and copied targets are refused before RPC", () => {
  const [target] = permissionTargets(source())
  const { api, request } = requester()
  for (const forged of [
    { ...target },
    { ...target, conversationId: second },
    { ...target, executionId: "turn-b" },
    { ...target, permissionId: "review-b" },
    JSON.parse(JSON.stringify(target)) as PermissionTarget,
  ]) {
    expect(() => api.answer(forged, "allow-a")).toThrow(TypeError)
    expect(() => api.cancel(forged, "withdrawn")).toThrow(TypeError)
  }
  expect(request).not.toHaveBeenCalled()
})

it("PT6: a choice offered by another review is refused before RPC", async () => {
  const [a, b] = permissionTargets(source())
  const { api, request } = requester()
  expect(() => api.answer(a!, "deny-b")).toThrow(TypeError)
  expect(() => api.answer(b!, "allow-a")).toThrow(TypeError)
  expect(request).not.toHaveBeenCalled()
  await api.answer(b!, "deny-b")
  expect(request).toHaveBeenCalledTimes(1)
})

// Checked by client:typecheck in CI; this function is deliberately not run.
function rejectedSignatures(api: ConversationApi, target: PermissionTarget) {
  // @ts-expect-error targets have a private brand; arbitrary ID tuples are not targets
  const forged: PermissionTarget = {
    conversationId: first,
    executionId: "turn-a",
    permissionId: "review-a",
  }
  // @ts-expect-error one field cannot be substituted independently
  target.permissionId = "review-b"
  // @ts-expect-error the old independent-identity answer API is gone
  void api.answer(first, "turn-a", "review-a", "allow-a")
  // @ts-expect-error the old independent-identity cancel API is gone
  void api.cancel(first, "turn-a", "review-a", "withdrawn")
  return forged
}
void rejectedSignatures
