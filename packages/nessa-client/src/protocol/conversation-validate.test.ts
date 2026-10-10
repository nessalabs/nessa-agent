import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

import {
  conversationId,
  conversationMutation,
  conversationReceipt,
  conversationReorder,
  conversationView,
} from "./conversation-validate.js"
import { bounds } from "../generated/product.js"

const DIGEST = `sha256:${"0".repeat(64)}`
function image(change: { size?: number } = {}) {
  return { digest: DIGEST, mimeType: "image/png", size: 3, ...change }
}

function view() {
  return {
    conversationId: "conversation",
    title: null,
    revision: "1",
    approvalMode: "ask",
    approvalModes: [
      {
        id: "ask",
        name: "Provider asks",
        description: "Uses the provider's native prompts.",
      },
    ],
    truncated: false,
    queueComplete: true,
    transcriptState: "complete",
    messages: [
      {
        executionId: "queued",
        userText: "hello",
        attachments: [],
        files: [],
        status: "queued",
        parts: [],
      },
      {
        executionId: "running",
        userText: "run",
        attachments: [],
        files: [],
        status: "running",
        parts: [{ offset: 0, kind: "tool", text: "", toolId: "tool", noticeId: "" }],
      },
    ],
    pending: [
      {
        executionId: "queued",
        text: "hello",
        attachments: [],
        files: [],
        mode: "queued",
      },
    ],
    permissions: [],
    questions: [],
    tools: [
      {
        executionId: "running",
        toolId: "tool",
        title: "Tool",
        kind: "execute",
        status: "running",
        details: "",
        input: "{}",
      },
    ],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "unknown",
        nativeHookSuppression: "unknown",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unsupported_not_implemented",
        policyEndTurn: "unsupported_not_implemented",
        policyCloseSession: "unsupported_not_implemented",
        incomingElicitation: "unsupported",
      },
    },
    lifecycle: { phase: "attached" },
  }
}

describe("conversation view agreement", () => {
  it("requires a current transcript state and refuses unconfirmed controls", () => {
    const missing: Record<string, unknown> = view()
    delete missing.transcriptState
    expect(() => conversationView(missing, "conversation")).toThrow()
    expect(() =>
      conversationView({ ...view(), transcriptState: "future" }, "conversation"),
    ).toThrow()
    for (const transcriptState of ["not_loaded", "partial", "stale", "unknown"]) {
      expect(() =>
        conversationView({ ...view(), transcriptState }, "conversation"),
      ).toThrow(/Unconfirmed/)
      const sample = view()
      const safe = {
        ...sample,
        transcriptState,
        queueComplete: false,
        pending: [],
        permissions: [],
        questions: [],
        capabilities: {
          ...sample.capabilities,
          queue: false,
          steer: false,
          permissions: false,
        },
      }
      expect(conversationView(safe, "conversation").transcriptState).toBe(transcriptState)
    }
    expect(() =>
      conversationView({ ...view(), transcriptState: "complete_empty" }, "conversation"),
    ).toThrow(/Empty conversation/)
  })

  it("keeps the product fixture's question attached to a running turn", () => {
    const fixtures = JSON.parse(
      readFileSync(
        new URL("../../../../protocol/product/fixtures.json", import.meta.url),
        "utf8",
      ),
    )
    const sample = fixtures.ConversationView
    const parsed = conversationView(sample, sample.conversationId)
    expect(parsed.questions[0]?.executionId).toBe(parsed.messages[0]?.executionId)
    expect(parsed.capabilities.agentFeatures.incomingElicitation).toBe(
      "supported_with_correlated_round_trip",
    )
  })

  it("accepts complete matching pending and tool evidence", () => {
    expect(conversationView(view(), "conversation").revision).toBe("1")
  })

  it("accepts a tool's MCP identity and structured result within the published bounds", () => {
    const atBound = `{"a":"${"x".repeat(bounds.maxMcpArgumentsBytes - 8)}"}`
    const value = view()
    Object.assign(value.tools[0]!, {
      mcp: {
        server: "é".repeat(bounds.maxMcpNameBytes / 2),
        tool: "show",
        resourceUri: `ui://${"é".repeat((bounds.maxUiResourceUriBytes - 6) / 2)}`,
        argumentsJson: atBound,
      },
      structuredContent: "a".repeat(bounds.maxToolStructuredContentBytes),
    })
    expect(conversationView(value, "conversation").tools[0]).toMatchObject({
      mcp: { tool: "show", argumentsJson: atBound },
    })
    // Without them the tool reads as it always has.
    expect(conversationView(view(), "conversation").tools[0]?.mcp).toBeUndefined()
  })

  it("rejects a tool's MCP identity or structured result outside the published bounds", () => {
    const mutations: Array<Record<string, unknown>> = [
      { mcp: { server: "", tool: "show" } },
      { mcp: { server: "charts", tool: "" } },
      { mcp: { server: "a".repeat(bounds.maxMcpNameBytes + 1), tool: "show" } },
      { mcp: { server: "charts", tool: "é".repeat(bounds.maxMcpNameBytes / 2 + 1) } },
      { mcp: { server: "charts" } },
      { mcp: { server: "charts", tool: "show", resourceUri: "" } },
      {
        mcp: {
          server: "charts",
          tool: "show",
          resourceUri: `ui://${"a".repeat(bounds.maxUiResourceUriBytes - 4)}`,
        },
      },
      { mcp: { server: "charts", tool: "show", other: "x" } },
      { mcp: "charts" },
      { structuredContent: "a".repeat(bounds.maxToolStructuredContentBytes + 1) },
      { structuredContent: { rows: 2 } },
      {
        mcp: {
          server: "charts",
          tool: "show",
          argumentsJson: `{"a":"${"x".repeat(bounds.maxMcpArgumentsBytes - 7)}"}`,
        },
      },
    ]
    for (const mutation of mutations) {
      const value = view()
      Object.assign(value.tools[0]!, mutation)
      expect(
        () => conversationView(value, "conversation"),
        JSON.stringify(mutation),
      ).toThrow()
    }
  })

  it("keeps the committed approval mode separate from a pending request", () => {
    const value = {
      ...view(),
      approvalModes: [
        { id: "ask", name: "Provider asks", description: "Native prompts." },
        { id: "auto", name: "Automatic review", description: "Provider review." },
      ],
      approvalModeChange: {
        requestId: "mode-auto",
        requestedMode: "auto",
        status: "recovery_required",
      },
    }
    const parsed = conversationView(value, "conversation")
    expect(parsed.approvalMode).toBe("ask")
    expect(parsed.approvalModeChange?.requestedMode).toBe("auto")
    value.approvalModeChange.status = "complete"
    expect(() => conversationView(value, "conversation")).toThrow(
      "Invalid conversation state",
    )
  })

  it("accepts one bounded local notice and rejects a repeated identity", () => {
    const value = view()
    value.messages[1]!.parts.push({
      offset: 1,
      kind: "local_notice",
      text: "Nessa declined.",
      toolId: "",
      noticeId: "1",
    })
    expect(conversationView(value, "conversation").messages[1]!.parts[1]!.noticeId).toBe(
      "1",
    )
    value.messages[1]!.parts.push({
      offset: 2,
      kind: "local_notice",
      text: "conflicting duplicate",
      toolId: "",
      noticeId: "1",
    })
    expect(() => conversationView(value, "conversation")).toThrow(
      "repeats a local notice identity",
    )
  })

  it("accepts a tool call as one part and rejects a second part for it", () => {
    const value = view()
    expect(conversationView(value, "conversation").messages[1]!.parts).toHaveLength(1)
    value.messages[1]!.parts.push({
      offset: 1,
      kind: "tool",
      text: "",
      toolId: "tool",
      noticeId: "",
    })
    expect(() => conversationView(value, "conversation")).toThrow(
      "repeats a tool call part",
    )
  })

  it("rejects a local notice identity outside the SDK sequence range", () => {
    const value = view()
    value.messages[1]!.parts.push({
      offset: 1,
      kind: "local_notice",
      text: "Nessa declined.",
      toolId: "",
      noticeId: "18446744073709551616",
    })
    expect(() => conversationView(value, "conversation")).toThrow(
      "local notice has no valid identity",
    )
  })

  it("rejects contradictory pending text when queue evidence is complete", () => {
    const value = view()
    value.pending[0]!.text = "different"
    expect(() => conversationView(value, "conversation")).toThrow(
      "Pending execution contradicts",
    )
  })

  it("accepts matching pending and message text above the former preview limit", () => {
    const value = view()
    const text = "😀".repeat(2048)
    value.messages[0]!.userText = text
    value.pending[0]!.text = text

    expect(new TextEncoder().encode(text)).toHaveLength(8192)
    expect(() => conversationView(value, "conversation")).not.toThrow()
  })

  it("K10: reads who wrote a turn, and holds the waiting input to the same author", () => {
    const written = {
      executionId: "running",
      toolId: "tool",
      server: "charts",
      tool: "show",
    }
    const value = view() as ReturnType<typeof view> & {
      messages: Record<string, unknown>[]
      pending: Record<string, unknown>[]
    }
    value.messages[0]!.app = written
    value.pending[0]!.app = written
    expect(conversationView(value, "conversation").messages[0]!.app).toEqual(written)

    // One submission seen twice: an app's turn waiting as the person's is
    // as contradictory as different text.
    const person = structuredClone(value)
    delete person.pending[0]!.app
    expect(() => conversationView(person, "conversation")).toThrow(
      "Pending execution contradicts",
    )
    const other = structuredClone(value)
    other.pending[0]!.app = { ...written, toolId: "other" }
    expect(() => conversationView(other, "conversation")).toThrow(
      "Pending execution contradicts",
    )
    for (const app of [
      { ...written, extra: 1 },
      { ...written, executionId: "" },
      { ...written, toolId: "x".repeat(257) },
      { ...written, server: "x".repeat(bounds.maxMcpNameBytes + 1) },
      { ...written, tool: "" },
      "charts",
      null,
    ]) {
      const invalid = structuredClone(value)
      invalid.messages[0]!.app = app
      expect(
        () => conversationView(invalid, "conversation"),
        JSON.stringify(app),
      ).toThrow()
    }
  })

  it("allows pending text mismatch when bounded evidence is truncated", () => {
    const value = view()
    value.truncated = true
    value.pending[0]!.text = "bounded preview"
    expect(() => conversationView(value, "conversation")).not.toThrow()
  })

  it("rejects orphan and cross-execution tool state in complete views", () => {
    const orphan = view()
    orphan.messages[1]!.parts = []
    expect(() => conversationView(orphan, "conversation")).toThrow("orphaned")

    const mismatch = view()
    mismatch.tools[0]!.executionId = "queued"
    expect(() => conversationView(mismatch, "conversation")).toThrow()
  })

  it("allows bounded tool omissions only when the view says it is truncated", () => {
    const value = view()
    value.truncated = true
    value.tools = []
    expect(() => conversationView(value, "conversation")).not.toThrow()
  })

  it("reads who asked for a review: the agent, or an app naming the tool it asked for", () => {
    const withOrigin = (origin: unknown) => {
      const value = view()
      Object.assign(value, {
        permissions: [
          {
            executionId: "running",
            permissionId: "permission",
            toolId: "tool",
            title: "Review",
            toolName: "delete_rows",
            argumentsJson: "{}",
            origin,
            ask: "tool",
            options: [{ id: "allow", label: "Allow", effect: "allow" }],
          },
        ],
      })
      return value
    }
    for (const origin of [
      { kind: "harness" },
      { kind: "app", server: "charts", tool: "delete_rows" },
    ])
      expect(() => conversationView(withOrigin(origin), "conversation")).not.toThrow()
    // An app asks from a tool call that has finished; the agent only while
    // its execution runs.
    const finished = (origin: unknown) => {
      const value = withOrigin(origin)
      const message = value.messages.find((each) => each.executionId === "running")!
      message.status = "completed"
      return value
    }
    expect(() =>
      conversationView(
        finished({ kind: "app", server: "charts", tool: "delete_rows" }),
        "conversation",
      ),
    ).not.toThrow()
    expect(() => conversationView(finished({ kind: "harness" }), "conversation")).toThrow(
      "not running",
    )
    for (const origin of [
      undefined,
      {},
      { kind: "plugin" },
      { kind: "app" },
      { kind: "app", server: "charts" },
      { kind: "app", tool: "delete_rows" },
      { kind: "app", server: "", tool: "delete_rows" },
      { kind: "app", server: "charts", tool: "" },
      { kind: "harness", tool: "delete_rows" },
      { kind: "app", server: "charts", tool: "delete_rows", extra: true },
    ])
      expect(() => conversationView(withOrigin(origin), "conversation")).toThrow()
  })

  it("refuses an app's review that names a tool other than the one it reviews", () => {
    const withTools = (tool: string, toolName: string) => {
      const value = view()
      Object.assign(value, {
        permissions: [
          {
            executionId: "running",
            permissionId: "permission",
            toolId: "tool",
            title: "Review",
            toolName,
            argumentsJson: "{}",
            origin: { kind: "app", server: "charts", tool },
            ask: "tool",
            options: [{ id: "allow", label: "Allow", effect: "allow" }],
          },
        ],
      })
      return value
    }
    expect(() =>
      conversationView(withTools("app_delete_rows", "app_delete_rows"), "conversation"),
    ).not.toThrow()
    for (const [tool, toolName] of [
      ["app_delete_row", "app_delete_rows"],
      ["App_Delete_Rows", "app_delete_rows"],
    ])
      expect(() => conversationView(withTools(tool, toolName), "conversation")).toThrow(
        "An app's review names a tool other than the one it reviews",
      )
  })

  it("D19 (#390): reads what a review asks as a closed set, and refuses a view whose review asks nothing it knows", () => {
    const withAsk = (ask: unknown, keep = true) => {
      const value = view()
      Object.assign(value, {
        permissions: [
          {
            executionId: "running",
            permissionId: "permission",
            toolId: "tool",
            title: "The show app on charts asks to send a message as you",
            toolName: "show",
            argumentsJson: '{"text":"Plot May"}',
            origin: { kind: "app", server: "charts", tool: "show" },
            ...(keep ? { ask } : {}),
            options: [{ id: "allow", label: "Allow", effect: "allow" }],
          },
        ],
      })
      return value
    }
    for (const ask of ["tool", "message"])
      expect(conversationView(withAsk(ask), "conversation").permissions[0]!.ask).toBe(ask)
    expect(() => conversationView(withAsk(undefined, false), "conversation")).toThrow()
    for (const ask of [null, "", "Message", "send_message", "tool ", 1, {}])
      expect(() => conversationView(withAsk(ask), "conversation")).toThrow()
  })

  it("D19 (#390): refuses a review the agent asked for that asks to send a message, a contradiction", () => {
    const withOrigin = (ask: string) => {
      const value = view()
      Object.assign(value, {
        permissions: [
          {
            executionId: "running",
            permissionId: "permission",
            toolId: "tool",
            title: "Run write_file",
            toolName: "write_file",
            argumentsJson: "{}",
            origin: { kind: "harness" },
            ask,
            options: [{ id: "allow", label: "Allow", effect: "allow" }],
          },
        ],
      })
      return value
    }
    expect(conversationView(withOrigin("tool"), "conversation").permissions[0]!.ask).toBe(
      "tool",
    )
    expect(() => conversationView(withOrigin("message"), "conversation")).toThrow(
      "A review the agent asked for asks to send a message",
    )
  })

  it("reads what each permission option decides, and refuses an option that does not say", () => {
    const withOptions = (options: unknown[]) => {
      const value = view()
      Object.assign(value, {
        permissions: [
          {
            executionId: "running",
            permissionId: "permission",
            toolId: "tool",
            title: "Review",
            toolName: "write_file",
            argumentsJson: "{}",
            origin: { kind: "harness" },
            ask: "tool",
            options,
          },
        ],
      })
      return value
    }
    const read = conversationView(
      withOptions([
        { id: "a", label: "Allow", effect: "allow" },
        { id: "d", label: "Deny", effect: "deny" },
      ]),
      "conversation",
    )
    expect(read.permissions[0]!.options.map((option) => option.effect)).toEqual([
      "allow",
      "deny",
    ])
    for (const option of [
      { id: "a", label: "Allow" },
      { id: "a", label: "Allow", effect: "always" },
      { id: "a", label: "Allow", effect: "" },
      { id: "a", label: "Allow", effect: true },
    ])
      expect(() => conversationView(withOptions([option]), "conversation")).toThrow()
  })

  it("rejects unknown fields at the view and nested schema boundaries", () => {
    const mutations: Array<(value: ReturnType<typeof view>) => void> = [
      (value) => Object.assign(value, { extra: true }),
      (value) => Object.assign(value.messages[0]!, { extra: true }),
      (value) => Object.assign(value.messages[1]!.parts[0]!, { extra: true }),
      (value) => Object.assign(value.pending[0]!, { extra: true }),
      (value) => Object.assign(value.tools[0]!, { extra: true }),
      (value) => Object.assign(value.capabilities, { extra: true }),
    ]
    for (const mutate of mutations) {
      const value = view()
      mutate(value)
      expect(() => conversationView(value, "conversation")).toThrow("unknown fields")
    }

    const permission = view()
    Object.assign(permission, {
      permissions: [
        {
          executionId: "running",
          permissionId: "permission",
          toolId: "tool",
          title: "Review",
          toolName: "write_file",
          argumentsJson: "{}",
          origin: { kind: "harness" },
          ask: "tool",
          options: [{ id: "allow", label: "Allow", effect: "allow", extra: true }],
        },
      ],
    })
    expect(() => conversationView(permission, "conversation")).toThrow("unknown fields")
  })

  it("accepts an image-only message whose waiting input names the same images", () => {
    const value = view()
    value.messages[0]!.userText = ""
    value.pending[0]!.text = ""
    Object.assign(value.messages[0]!, { attachments: [image()] })
    // Field order belongs to the serializer; the same image is the same image.
    Object.assign(value.pending[0]!, {
      attachments: [{ size: 3, mimeType: "image/png", digest: DIGEST }],
      files: [],
    })
    const checked = conversationView(value, "conversation")
    expect(checked.messages[0]!.attachments).toEqual([image()])
  })

  it("rejects waiting images that contradict their queued message", () => {
    const value = view()
    Object.assign(value.messages[0]!, { attachments: [image()] })
    Object.assign(value.pending[0]!, { attachments: [image({ size: 4 })] })
    expect(() => conversationView(value, "conversation")).toThrow(
      "Pending execution contradicts",
    )
    const bounded = view()
    bounded.truncated = true
    Object.assign(bounded.pending[0]!, { attachments: [image()] })
    expect(() => conversationView(bounded, "conversation")).not.toThrow()
  })

  it.each([
    ["an uppercase digest", { digest: `sha256:${"A".repeat(64)}` }],
    ["a bare hex digest", { digest: "0".repeat(64) }],
    ["a short digest", { digest: "sha256:00" }],
    ["a media type no message may carry", { mimeType: "image/svg+xml" }],
    ["a non-image media type", { mimeType: "application/pdf" }],
    ["an empty image", { size: 0 }],
    ["a fractional size", { size: 1.5 }],
    ["an image one byte over the schema's maximum", { size: 5_242_881 }],
    ["an unknown field", { bytes: "AAAA" }],
  ])("rejects a message image with %s", (_name, change) => {
    for (const where of ["messages", "pending"] as const) {
      const value = view()
      value.truncated = true
      Object.assign(value[where][0]!, { attachments: [{ ...image(), ...change }] })
      expect(() => conversationView(value, "conversation")).toThrow("attachments")
    }
  })

  it("rejects more images, or more image bytes, than one message may carry", () => {
    const eleven = view()
    eleven.truncated = true
    Object.assign(eleven.messages[0]!, {
      attachments: Array.from({ length: 11 }, () => image()),
    })
    expect(() => conversationView(eleven, "conversation")).toThrow("at most 10")

    const heavy = view()
    heavy.truncated = true
    Object.assign(heavy.messages[0]!, {
      attachments: Array.from({ length: 3 }, () => image({ size: 4 * 1024 * 1024 })),
    })
    expect(() => conversationView(heavy, "conversation")).toThrow("bytes of images")

    const exact = view()
    exact.truncated = true
    Object.assign(exact.messages[0]!, {
      attachments: Array.from({ length: 2 }, () => image({ size: 5 * 1024 * 1024 })),
    })
    expect(() => conversationView(exact, "conversation")).not.toThrow()
  })

  it("requires attachments and the image capability rather than assuming them", () => {
    const message = view()
    delete (message.messages[0] as { attachments?: unknown }).attachments
    expect(() => conversationView(message, "conversation")).toThrow("attachments")

    const pending = view()
    delete (pending.pending[0] as { attachments?: unknown }).attachments
    expect(() => conversationView(pending, "conversation")).toThrow("attachments")

    const capability = view()
    delete (capability.capabilities as { imageInput?: boolean }).imageInput
    expect(() => conversationView(capability, "conversation")).toThrow("imageInput")

    const truthy = view()
    Object.assign(truthy.capabilities, { imageInput: "true" })
    expect(() => conversationView(truthy, "conversation")).toThrow("imageInput")

    const missingFeatures = view()
    delete (missingFeatures.capabilities as { agentFeatures?: unknown }).agentFeatures
    expect(() => conversationView(missingFeatures, "conversation")).toThrow(
      "Invalid conversation response",
    )

    const contradictoryFeature = view()
    Object.assign(contradictoryFeature.capabilities.agentFeatures, {
      preToolPolicy: "supported_for_offered_permission_reviews",
    })
    expect(() => conversationView(contradictoryFeature, "conversation")).toThrow(
      "Invalid conversation state",
    )
  })

  it("accepts the single current interaction display notice contract", () => {
    const value = {
      ...view(),
      interactionViewError: "Use Stop to cancel pending interactions.",
    }
    expect(conversationView(value, "conversation").interactionViewError).toBe(
      value.interactionViewError,
    )
    expect(() =>
      conversationView({ ...view(), permissionViewError: "old field" }, "conversation"),
    ).toThrow()
  })

  it("rejects unknown fields in receipts and control results", () => {
    expect(() => conversationId({ conversationId: "c", extra: true }, "c")).toThrow()
    expect(() =>
      conversationReceipt({ executionId: "e", disposition: "queued", extra: true }, "e"),
    ).toThrow()
    expect(() =>
      conversationMutation({ requestId: "r", applied: true, extra: true }, "r"),
    ).toThrow()
    expect(() =>
      conversationReorder({ requestId: "r", outcome: "applied", extra: true }, "r"),
    ).toThrow()
  })
})

describe("a tool's kind", () => {
  const withKind = (kind: unknown) => {
    const value = view()
    value.tools = [{ ...value.tools[0]!, kind } as (typeof value.tools)[number]]
    return value
  }

  it.each(["", "read", "edit", "search", "execute", "switch_mode", "other"])(
    "accepts the provider's category %j",
    (kind) => {
      expect(() => conversationView(withKind(kind), "conversation")).not.toThrow()
    },
  )

  it.each([["guess"], [7], [null]])(
    "rejects %j, which the schema does not name",
    (kind) => {
      expect(() => conversationView(withKind(kind), "conversation")).toThrow()
    },
  )

  it("rejects a tool that leaves it out", () => {
    const value = view()
    const { kind: _kind, ...tool } = value.tools[0]!
    value.tools = [tool as (typeof value.tools)[number]]
    // Required, not optional: a gateway that stopped sending it has changed shape.
    expect(() => conversationView(value, "conversation")).toThrow(
      "Invalid conversation kind",
    )
  })
})

it("accepts typed authentication only on a failed turn and rejects untyped values", () => {
  const sample = view()
  const refused = {
    ...sample,
    messages: [
      {
        ...sample.messages[1],
        parts: [],
        status: "failed",
        authenticationRequired: true,
      },
    ],
    pending: [],
    permissions: [],
    questions: [],
    tools: [],
  }
  expect(
    conversationView(refused, "conversation").messages[0].authenticationRequired,
  ).toBe(true)
  expect(() =>
    conversationView(
      { ...refused, messages: [{ ...refused.messages[0], status: "completed" }] },
      "conversation",
    ),
  ).toThrow(/authentication refusal/)
  expect(() =>
    conversationView(
      {
        ...refused,
        messages: [{ ...refused.messages[0], authenticationRequired: "true" }],
      },
      "conversation",
    ),
  ).toThrow(/authenticationRequired/)
})

function protocolFile(path: string) {
  return JSON.parse(
    readFileSync(
      new URL(`../../../../protocol/product/${path}`, import.meta.url),
      "utf8",
    ),
  )
}

describe("the view's lease", () => {
  const withLease = (lease: unknown) => ({ ...view(), lease })

  it("accepts every state the schema publishes, with the fields that go with it", () => {
    const leases = [
      {
        state: "live",
        revision: 1,
        environment: "here",
        sandbox: "harness_default",
        droppedEvents: 0,
      },
      {
        state: "live",
        revision: 1,
        environment: "here",
        sandbox: "none",
        droppedEvents: 0,
        commands: 4,
      },
      {
        state: "ending",
        revision: 2,
        environment: "here",
        sandbox: "harness_default",
        cause: "closed",
        droppedEvents: 0,
        commands: 1,
      },
      {
        state: "ended",
        revision: 2,
        environment: "here",
        sandbox: "harness_default",
        cause: "lost",
        cleanup: "not_held",
        droppedEvents: 3,
      },
      {
        state: "interrupted",
        revision: 2,
        environment: "here",
        sandbox: "harness_default",
        cause: "stopped",
        cleanup: "forced",
        droppedEvents: 0,
      },
      {
        state: "interrupted",
        revision: 2,
        environment: "here",
        sandbox: "harness_default",
        cause: "lost",
        droppedEvents: 1,
      },
      {
        state: "refused",
        revision: 1,
        environment: "here",
        sandbox: "harness_default",
        refusal: "sandbox_unavailable",
        droppedEvents: 0,
      },
      { state: "unreadable", droppedEvents: 0 },
    ]
    for (const lease of leases)
      expect(conversationView(withLease(lease), "conversation").lease).toEqual(lease)
    expect(conversationView(view(), "conversation").lease).toBeUndefined()
  })

  const live = {
    state: "live",
    revision: 1,
    environment: "here",
    sandbox: "harness_default",
    droppedEvents: 0,
  }
  const ended = { ...live, state: "ended", cause: "closed", cleanup: "confirmed" }
  const refusedLease = {
    ...live,
    state: "refused",
    refusal: "sandbox_unavailable",
  }

  it("refuses anything outside the published shape", () => {
    expect(() =>
      conversationView(withLease({ ...live, holder: "elsewhere" }), "conversation"),
    ).toThrow(/unknown fields/)
    const refused: [unknown, RegExp][] = [
      [{ state: "paused", droppedEvents: 0 }, /lease state/],
      [{ droppedEvents: 0 }, /lease state/],
      [{ ...live, environment: "remote" }, /lease environment/],
      [{ ...live, sandbox: "container" }, /lease sandbox/],
      [{ ...live, cause: "toString" }, /lease cause/],
      [{ ...live, cleanup: "no_process" }, /lease cleanup/],
      [{ ...live, refusal: "busy" }, /lease refusal/],
      [{ ...live, revision: 0 }, /lease revision/],
      [{ ...live, revision: 1.5 }, /lease revision/],
      [{ state: "live" }, /lease droppedEvents/],
      [{ ...live, droppedEvents: -1 }, /lease droppedEvents/],
      [{ ...live, droppedEvents: Number.MAX_SAFE_INTEGER + 1 }, /lease droppedEvents/],
      [{ ...live, commands: 0 }, /lease commands/],
      [{ ...live, commands: 5 }, /lease commands/],
      [{ ...live, commands: 1.5 }, /lease commands/],
      [null, /Invalid conversation response/],
    ]
    for (const [lease, error] of refused)
      expect(() => conversationView(withLease(lease), "conversation")).toThrow(error)
  })

  it("refuses a lease whose fields contradict its state", () => {
    const { revision: _revision, ...unnumbered } = live
    const { cause: _cause, ...causeless } = ended
    const { cleanup: _cleanup, ...uncleaned } = ended
    const { refusal: _refusal, ...unexplained } = refusedLease
    const contradictions: [unknown, RegExp][] = [
      [{ ...live, cause: "closed" }, /lease cause/],
      [{ ...live, cleanup: "confirmed" }, /lease cleanup/],
      [{ ...live, droppedEvents: 1 }, /lease droppedEvents/],
      [{ ...ended, commands: 1 }, /lease commands/],
      [{ ...refusedLease, commands: 1 }, /lease commands/],
      [unnumbered, /lease revision/],
      [{ ...live, state: "ending" }, /lease cause/],
      [
        { ...live, state: "ending", cause: "closed", cleanup: "confirmed" },
        /lease cleanup/,
      ],
      [
        { ...live, state: "ending", cause: "closed", droppedEvents: 2 },
        /lease droppedEvents/,
      ],
      [causeless, /lease cause/],
      [uncleaned, /lease cleanup/],
      [{ ...ended, refusal: "sandbox_unavailable" }, /lease refusal/],
      [{ ...live, state: "interrupted" }, /lease cause/],
      [unexplained, /lease refusal/],
      [{ ...refusedLease, cause: "closed" }, /lease cause/],
      [{ ...refusedLease, droppedEvents: 1 }, /lease droppedEvents/],
      [{ state: "unreadable", revision: 4, droppedEvents: 0 }, /lease revision/],
      [
        { state: "unreadable", environment: "here", droppedEvents: 0 },
        /lease environment/,
      ],
      [{ state: "unreadable", droppedEvents: 1 }, /lease droppedEvents/],
    ]
    for (const [lease, error] of contradictions)
      expect(
        () => conversationView(withLease(lease), "conversation"),
        JSON.stringify(lease),
      ).toThrow(error)
  })

  it("knows every field and value the schema gives a view and its lease", () => {
    const defs = protocolFile("v1.json").$defs
    // A field the schema adds to the view is one the validator must know, or
    // every view that carries it is refused as unknown.
    for (const key of Object.keys(defs.ConversationView.properties)) {
      let error: unknown
      try {
        conversationView({ ...view(), [key]: undefined }, "conversation")
      } catch (caught) {
        error = caught
      }
      expect(String(error), key).not.toMatch(/unknown fields/)
    }
    const properties = defs.ConversationLease.properties as Record<
      string,
      { enum?: string[] }
    >
    // Each value on a lease whose state carries that field.
    const carriers: Record<string, Record<string, unknown>> = {
      cause: ended,
      cleanup: ended,
      refusal: refusedLease,
    }
    const states: Record<string, Record<string, unknown>> = {
      live,
      ending: { ...live, state: "ending", cause: "closed" },
      ended,
      interrupted: { ...live, state: "interrupted", cause: "lost" },
      refused: refusedLease,
      unreadable: { state: "unreadable", droppedEvents: 0 },
    }
    for (const [key, schema] of Object.entries(properties)) {
      for (const value of schema.enum ?? []) {
        // An SSH environment names its host.
        const host = key === "environment" && value === "ssh" ? { host: "devbox" } : {}
        const sample =
          key === "state"
            ? states[value]
            : { ...(carriers[key] ?? live), [key]: value, ...host }
        expect(sample, value).toBeDefined()
        expect(conversationView(withLease(sample), "conversation").lease).toEqual(sample)
      }
    }
  })

  it("holds a lease's host to its SSH environment", () => {
    const there = { ...live, environment: "ssh", host: "me@devbox" }
    expect(conversationView(withLease(there), "conversation").lease).toEqual(there)
    for (const contradiction of [
      { ...live, environment: "ssh" },
      { ...live, environment: "ssh", host: "" },
      { ...live, environment: "ssh", host: "d".repeat(254) },
      { ...live, environment: "here", host: "devbox" },
    ])
      expect(() => conversationView(withLease(contradiction), "conversation")).toThrow()
  })

  it("accepts the view a real gateway serves for a leased conversation", () => {
    // The gateway's own conversation.read answer, written by its test
    // (`read_joins_the_fixed_selection_to_the_authenticated_catalog`), never by hand.
    const served = protocolFile("samples/conversation-read.json")
    const parsed = conversationView(served, served.conversationId)
    expect(parsed.lease?.state).toBe("live")
  })
})
