import { describe, expect, it, vi } from "vitest"
import Ajv from "ajv"
import {
  createConversationReadTools,
  NessaConnectionClosedError,
  NessaRpcError,
  NessaSessionUnavailableError,
  type ConversationApi,
  type ConversationListResult,
  type ConversationView,
} from "../index.js"
import { conversationIdPattern } from "../protocol/conversation-validate.js"

const conversationId = "00000000-0000-4000-8000-000000000001"
const listed: ConversationListResult = { conversations: [], complete: false }

function setup() {
  const list = vi.fn<ConversationApi["list"]>().mockResolvedValue(listed)
  const read = vi.fn<ConversationApi["read"]>().mockResolvedValue(view)
  return {
    list,
    read,
    adapter: createConversationReadTools({ conversation: { list, read } }),
  }
}

it("publishes two immutable JSON Schema descriptors using the UUID owner's pattern", () => {
  const { adapter, list, read } = setup()
  expect(adapter.tools.map((tool) => tool.name)).toEqual([
    "nessa_conversations_list",
    "nessa_conversation_read",
  ])
  const ajv = new Ajv()
  for (const tool of adapter.tools) {
    expect(Object.isFrozen(tool)).toBe(true)
    expect(Object.isFrozen(tool.inputSchema)).toBe(true)
    expect(Object.isFrozen(tool.inputSchema.properties)).toBe(true)
    expect(Object.isFrozen(tool.inputSchema.required)).toBe(true)
    for (const property of Object.values(tool.inputSchema.properties))
      expect(Object.isFrozen(property)).toBe(true)
    const validate = ajv.compile(tool.inputSchema)
    const args = tool.name === "nessa_conversations_list" ? {} : { conversationId }
    expect(validate(args)).toBe(true)
    expect(validate({ ...args, url: "ws://elsewhere" })).toBe(false)
    if (tool.name === "nessa_conversation_read") {
      expect(tool.inputSchema.properties.conversationId.pattern).toBe(
        conversationIdPattern.source,
      )
      expect(validate({ conversationId: "invalid" })).toBe(false)
      expect(validate({})).toBe(false)
    } else {
      expect(validate({ archived: "true" })).toBe(false)
    }
  }
  expect(Object.isFrozen(adapter.tools)).toBe(true)
  expect(JSON.parse(JSON.stringify(adapter.tools))).toEqual(adapter.tools)
  expect(list).not.toHaveBeenCalled()
  expect(read).not.toHaveBeenCalled()
})

it("dispatches both reads once and preserves results including incomplete evidence", async () => {
  const { adapter, list, read } = setup()
  for (const args of [{}, { archived: false }, { archived: true }, Object.create(null)])
    expect(await adapter.call("nessa_conversations_list", args)).toBe(listed)
  expect(list.mock.calls).toEqual([
    [{}],
    [{ archived: false }],
    [{ archived: true }],
    [{}],
  ])
  expect(await adapter.call("nessa_conversation_read", { conversationId })).toBe(view)
  expect(read.mock.calls).toEqual([[conversationId]])
  expect(await adapter.call("nessa_conversation_read", { conversationId })).toBe(view)
  expect(read).toHaveBeenCalledTimes(2)
})

describe("rejects malformed arguments before API effects", () => {
  const invalidObjects: unknown[] = [
    undefined,
    null,
    false,
    0,
    "{}",
    [],
    new Date(),
    Object.create({ archived: true }),
    Object.setPrototypeOf([], Object.prototype),
    Object.setPrototypeOf(() => undefined, Object.prototype),
  ]
  it.each(invalidObjects.map((args, index) => ({ args, index })))(
    "rejects object shape $index",
    async ({ args }) => {
      const { adapter, list, read } = setup()
      for (const name of ["nessa_conversations_list", "nessa_conversation_read"])
        await expect(adapter.call(name, args)).rejects.toBeInstanceOf(TypeError)
      expect(list).not.toHaveBeenCalled()
      expect(read).not.toHaveBeenCalled()
    },
  )

  it.each([
    ["nessa_conversations_list", { archived: undefined }],
    ["nessa_conversations_list", { archived: null }],
    ["nessa_conversations_list", { archived: "true" }],
    ["nessa_conversations_list", { archived: 1 }],
    ["nessa_conversations_list", { conversationId }],
    ["nessa_conversation_read", {}],
    ["nessa_conversation_read", { conversationId: 12 }],
    ["nessa_conversation_read", { conversationId: null }],
    ["nessa_conversation_read", { conversationId: { toString: () => conversationId } }],
    ["nessa_conversation_read", { conversationId: "not-a-uuid" }],
    [
      "nessa_conversation_read",
      { conversationId: "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA" },
    ],
    ["nessa_conversation_read", { conversationId, archived: false }],
  ])("rejects fields for %s", async (name, args) => {
    const { adapter, list, read } = setup()
    await expect(adapter.call(name, args)).rejects.toBeInstanceOf(TypeError)
    expect(list).not.toHaveBeenCalled()
    expect(read).not.toHaveBeenCalled()
  })

  it("rejects credentials, endpoints, prototype names, hidden fields and symbols", async () => {
    const { adapter, list, read } = setup()
    for (const name of ["nessa_conversations_list", "nessa_conversation_read"]) {
      const base = name === "nessa_conversation_read" ? { conversationId } : {}
      for (const field of [
        "url",
        "credential",
        "auth",
        "constructor",
        "toString",
        "__proto__",
        Symbol("extra"),
      ])
        await expect(
          adapter.call(name, { ...base, [field]: "untrusted" }),
        ).rejects.toBeInstanceOf(TypeError)
      await expect(
        adapter.call(name, Object.defineProperty({ ...base }, "hidden", { value: true })),
      ).rejects.toBeInstanceOf(TypeError)
    }
    expect(list).not.toHaveBeenCalled()
    expect(read).not.toHaveBeenCalled()
  })

  it("rejects accessor fields without invoking them", async () => {
    const { adapter, list, read } = setup()
    const getter = vi.fn(() => true)
    await expect(
      adapter.call(
        "nessa_conversations_list",
        Object.defineProperty({}, "archived", { get: getter }),
      ),
    ).rejects.toBeInstanceOf(TypeError)
    expect(getter).not.toHaveBeenCalled()
    expect(list).not.toHaveBeenCalled()
    expect(read).not.toHaveBeenCalled()
  })
})

it.each([
  "",
  "constructor",
  "toString",
  "__proto__",
  "conversation.read",
  "nessa_conversation_delete",
])("rejects unknown tool %s before effects", async (name) => {
  const { adapter, list, read } = setup()
  await expect(adapter.call(name, {})).rejects.toBeInstanceOf(TypeError)
  expect(list).not.toHaveBeenCalled()
  expect(read).not.toHaveBeenCalled()
})

const failures = [
  new NessaRpcError("permission_denied", "denied", { policies: [] }),
  new NessaConnectionClosedError(4002, JSON.stringify({ code: "credential_revoked" })),
  new NessaSessionUnavailableError(),
]
it.each(failures)(
  "preserves $name identity without replay and permits a subsequent recovery",
  async (error) => {
    const { adapter, list, read } = setup()
    list.mockRejectedValueOnce(error)
    await expect(adapter.call("nessa_conversations_list", {})).rejects.toBe(error)
    expect(list).toHaveBeenCalledTimes(1)
    expect(await adapter.call("nessa_conversations_list", {})).toBe(listed)
    expect(list).toHaveBeenCalledTimes(2)
    read.mockRejectedValueOnce(error)
    await expect(
      adapter.call("nessa_conversation_read", { conversationId }),
    ).rejects.toBe(error)
    expect(read).toHaveBeenCalledTimes(1)
    expect(await adapter.call("nessa_conversation_read", { conversationId })).toBe(view)
    expect(read).toHaveBeenCalledTimes(2)
  },
)
const view: ConversationView = {
  conversationId,
  artifacts: [],
  notes: [],
  unreadable: [],
  unreadableTruncated: false,
  handoffs: [],
  approvalMode: "ask",
  model: "claude-sonnet-5",
  approvalModes: [
    {
      id: "ask",
      name: "Provider asks",
      description: "The provider requests approval where required.",
    },
  ],
  title: "Flights to Lisbon",
  revision: "opaque-1",
  serverNowMs: 1_700_000_000_000,
  messages: [],
  pending: [],
  permissions: [],
  questions: [],
  tools: [],
  capabilities: {
    queue: true,
    steer: true,
    resume: true,
    permissions: true,
    imageInput: false,
    agentFeatures: {
      permissionDenial: "unknown",
      nativeHookSuppression: "unknown",
      nativeHooks: "off",
      compactionReporting: "unsupported_not_implemented",
      modelSwitchReporting: "unsupported_not_implemented",
      permissionDeferral: "unsupported_not_implemented",
      elicitationForwarding: "unknown",
      preToolPolicy: "unsupported_not_implemented",
      policyEndTurn: "unsupported",
      policyCloseSession: "unsupported",
      incomingElicitation: "unsupported",
    },
  },
  lifecycle: { phase: "attached" },
  truncated: false,
  earlier: false,
  queueComplete: true,
  transcriptState: "complete",
}
