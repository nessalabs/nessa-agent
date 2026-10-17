import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

import {
  conversationId,
  conversationMutation,
  conversationReceipt,
  conversationReorder,
  conversationView,
  maxPolicyRevision,
  minExpectedPolicyRevision,
  minPolicyRevision,
  conversationPermission,
  conversationQuestion,
  permissionReviewRange,
} from "./conversation-validate.js"
import { bounds } from "../generated/product.js"

const DIGEST = `sha256:${"0".repeat(64)}`
function entry(index = 0, origin = "harness", toolName = "write_file") {
  return { review: index.toString(16).padStart(32, "0"), origin, toolName }
}
function image(change: { size?: number } = {}) {
  return { digest: DIGEST, mimeType: "image/png", size: 3, ...change }
}

function view() {
  return {
    conversationId: "conversation",
    title: null,
    revision: "1",
    serverNowMs: 1_700_000_000_000,
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
    artifacts: [],
    notes: [],
    unreadable: [],
    unreadableTruncated: false,
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
        nativeHooks: "off",
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
  }
}

describe("conversation view agreement", () => {
  it("accepts a tool with no recorded hooks choice and refuses an unknown one", () => {
    const base = view() as { tools: Record<string, unknown>[] }
    const { nativeHooks: _, ...unmarked } = base.tools[0]
    expect(
      conversationView({ ...base, tools: [unmarked] }, "conversation").tools[0]
        .nativeHooks,
    ).toBeUndefined()
    expect(() =>
      conversationView(
        { ...base, tools: [{ ...base.tools[0], nativeHooks: "yes" }] },
        "conversation",
      ),
    ).toThrow("Invalid native hooks choice")
  })

  it("refuses two hooks choices for one execution", () => {
    const base = view() as {
      tools: Record<string, unknown>[]
      messages: { parts: Record<string, unknown>[] }[]
    }
    base.messages[1].parts.push({
      offset: 1,
      kind: "tool",
      text: "",
      toolId: "second",
      noticeId: "",
    })
    const second = { ...base.tools[0], toolId: "second" }
    expect(
      conversationView(
        { ...base, tools: [base.tools[0], { ...second, nativeHooks: "off" }] },
        "conversation",
      ).tools,
    ).toHaveLength(2)
    expect(() =>
      conversationView(
        { ...base, tools: [base.tools[0], { ...second, nativeHooks: "on" }] },
        "conversation",
      ),
    ).toThrow("An execution names two native hooks choices")
  })

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

  it("keeps a line for a record it could not read, including when the chat is otherwise empty", () => {
    const sample = view()
    const empty = {
      ...sample,
      messages: [],
      pending: [],
      tools: [],
      transcriptState: "complete_empty",
      unreadable: [{ id: "4" }, { id: "12", afterExecutionId: "run-1" }],
    }
    expect(conversationView(empty, "conversation").unreadable).toEqual([
      { id: "4" },
      { id: "12", afterExecutionId: "run-1" },
    ])
    expect(conversationView(empty, "conversation").unreadableTruncated).toBe(false)
    expect(
      conversationView({ ...empty, unreadableTruncated: true }, "conversation")
        .unreadableTruncated,
    ).toBe(true)
    expect(() =>
      conversationView({ ...empty, unreadableTruncated: "yes" }, "conversation"),
    ).toThrow(/unreadableTruncated/)
    expect(() =>
      conversationView({ ...empty, unreadable: [{ id: "01" }] }, "conversation"),
    ).toThrow(/unreadable id/)
    expect(() =>
      conversationView(
        { ...empty, unreadable: [{ id: "4" }, { id: "4" }] },
        "conversation",
      ),
    ).toThrow(/Repeated conversation unreadable id/)
    expect(() =>
      conversationView(
        { ...empty, unreadable: [{ id: "4", report: true }] },
        "conversation",
      ),
    ).toThrow(/unknown fields/)
    const tooMany = Array.from({ length: 33 }, (_, index) => ({ id: String(index) }))
    expect(() =>
      conversationView({ ...empty, unreadable: tooMany }, "conversation"),
    ).toThrow(/unreadable/)
  })

  it("lists the product fixture's question by its entry", () => {
    const fixtures = JSON.parse(
      readFileSync(
        new URL("../../../../protocol/product/fixtures.json", import.meta.url),
        "utf8",
      ),
    )
    const sample = fixtures.ConversationView
    const parsed = conversationView(sample, sample.conversationId)
    expect(parsed.questions[0]?.question).toMatch(/^[0-9a-f]{32}$/)
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
      return {
        review: "0".repeat(32),
        executionId: "running",
        permissionId: "permission",
        toolId: "tool",
        title: "Review",
        toolName: "delete_rows",
        argumentsJson: "{}",
        origin,
        ask: "tool",
        options: [{ id: "allow", label: "Allow", effect: "allow" }],
      }
    }
    for (const origin of [
      { kind: "harness" },
      { kind: "app", server: "charts", tool: "delete_rows" },
    ])
      expect(() => conversationPermission(withOrigin(origin))).not.toThrow()
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
      expect(() => conversationPermission(withOrigin(origin))).toThrow()
  })

  it("reads a review's whole input, whatever its size, as the agent gave it", () => {
    const withInput = (argumentsJson: string) => ({
      review: "0".repeat(32),
      executionId: "running",
      permissionId: "permission",
      toolId: "tool",
      title: "Write",
      toolName: "Write",
      argumentsJson,
      origin: { kind: "harness" },
      ask: "tool",
      options: [{ id: "allow", label: "Allow", effect: "allow" }],
    })
    // Not JSON, and need not be; and a 100 KB Write is read whole.
    for (const input of [
      '{"content":"é',
      JSON.stringify({ content: "x".repeat(100_000) }),
    ])
      expect(conversationPermission(withInput(input)).argumentsJson).toBe(input)
    // A review says only what it shows: no account of a part left out.
    expect(() =>
      conversationPermission({ ...withInput("{}"), argumentsComplete: true }),
    ).toThrow("unknown fields")
    expect(() =>
      conversationPermission({ ...withInput("{}"), argumentsBytes: 2 }),
    ).toThrow("unknown fields")
  })

  it("lists each waiting question by a small entry under its key, and reads it whole apart", () => {
    const listing = (questions: unknown[]) => {
      const value: Record<string, unknown> = view()
      value.questions = questions
      return () => conversationView(value, "conversation")
    }
    const entry = (index: number) => ({
      question: index.toString(16).padStart(32, "0"),
      message: "é".repeat(bounds.maxQuestionEntryMessageBytes / 2),
    })
    const most = Array.from({ length: bounds.maxOpenQuestions }, (_, index) =>
      entry(index),
    )
    expect(listing(most)()).toMatchObject({ questions: most })
    expect(listing([...most, entry(most.length)])).toThrow()
    expect(listing([entry(1), entry(1)])).toThrow("repeats a question")
    expect(listing([{ ...entry(1), question: "Q".repeat(32) }])).toThrow("question key")
    expect(listing([{ ...entry(1), message: "x".repeat(49) }])).toThrow()
    expect(listing([{ ...entry(1), executionId: "running" }])).toThrow("unknown fields")
    const asked = {
      question: "0".repeat(32),
      executionId: "running",
      questionId: "ask",
      message: "Which?",
      questions: [
        {
          key: "environment",
          prompt: "Where?",
          header: null,
          multiSelect: false,
          freeText: false,
          required: true,
          options: [{ value: "staging", label: "Staging", description: null }],
        },
      ],
    }
    expect(conversationQuestion(asked)).toEqual(asked)
    expect(() => conversationQuestion({ ...asked, question: "ask" })).toThrow(
      "question key",
    )
    expect(() => conversationQuestion({ ...asked, questions: [] })).toThrow(
      "asks nothing",
    )
    const { question: _key, ...unkeyed } = asked
    expect(() => conversationQuestion(unkeyed)).toThrow()
  })

  it("refuses a review shown without the key it is listed under", () => {
    const shown = {
      review: "0".repeat(32),
      executionId: "running",
      permissionId: "permission",
      toolId: "tool",
      title: "Write",
      toolName: "Write",
      argumentsJson: "{}",
      origin: { kind: "harness" },
      ask: "tool",
      options: [{ id: "allow", label: "Allow", effect: "allow" }],
    }
    expect(conversationPermission(shown).review).toBe(shown.review)
    const { review: _key, ...unkeyed } = shown
    expect(() => conversationPermission(unkeyed)).toThrow()
    expect(() => conversationPermission({ ...shown, review: "permission" })).toThrow(
      "review key",
    )
  })

  it("reads one range of a review, held to the range asked", () => {
    const data = (text: string) => btoa(text)
    expect(
      permissionReviewRange({ offset: 0, totalBytes: 5, data: data("hello") }, 0, 8),
    ).toMatchObject({ totalBytes: 5, bytes: new TextEncoder().encode("hello") })
    expect(
      permissionReviewRange({ offset: 2, totalBytes: 5, data: data("ll") }, 2, 2, 5)
        .bytes,
    ).toEqual(new TextEncoder().encode("ll"))
    for (const [value, offset, maxBytes, totalBytes] of [
      // Another range, a short or long one, a length that moved.
      [{ offset: 1, totalBytes: 5, data: data("hello") }, 0, 8, undefined],
      [{ offset: 0, totalBytes: 5, data: data("hell") }, 0, 8, undefined],
      [{ offset: 0, totalBytes: 5, data: data("hello!") }, 0, 8, undefined],
      [{ offset: 0, totalBytes: 6, data: data("hello") }, 0, 8, 5],
      [{ offset: 5, totalBytes: 5, data: "" }, 5, 8, undefined],
      // A review is never empty, nor longer than the most one can be.
      [{ offset: 0, totalBytes: 0, data: "" }, 0, 8, undefined],
      [{ offset: 0, totalBytes: bounds.maxReviewBytes + 1, data: "" }, 0, 8, undefined],
      [{ offset: 0, totalBytes: -1, data: "" }, 0, 8, undefined],
      [{ offset: 0, totalBytes: 5, data: data("hello"), extra: 1 }, 0, 8, undefined],
    ] as const)
      expect(() => permissionReviewRange(value, offset, maxBytes, totalBytes)).toThrow()
  })

  it("reads a review's deadline and refuses one it cannot show", () => {
    const withDeadline = (deadline: unknown) => {
      return {
        review: "0".repeat(32),
        executionId: "running",
        permissionId: "permission",
        toolId: "tool",
        title: "Review",
        toolName: "Write",
        argumentsJson: "{}",
        origin: { kind: "harness" },
        ask: "tool",
        options: [{ id: "deny", label: "Deny", effect: "deny" }],
        deadline,
      }
    }
    const reason =
      "No human answered in time. Continue without this tool or try another approach."
    const deadline = {
      waitMs: 30_000,
      dueAtMs: 1_700_000_030_000,
      effect: "deny" as const,
      reason,
      stopsTurn: false,
    }
    const read = conversationPermission(withDeadline(deadline))
    expect(read.deadline).toEqual(deadline)
    expect(
      conversationPermission(withDeadline({ ...deadline, stopsTurn: true })).deadline
        ?.stopsTurn,
    ).toBe(true)
    const allowed = { ...deadline, effect: "allow" as const, stopsTurn: false }
    expect(conversationPermission(withDeadline(allowed)).deadline).toEqual(allowed)
    for (const broken of [
      { waitMs: 0, dueAtMs: deadline.dueAtMs, effect: "deny", reason, stopsTurn: false },
      {
        waitMs: 86_400_001,
        dueAtMs: deadline.dueAtMs,
        effect: "deny",
        reason,
        stopsTurn: false,
      },
      { waitMs: 30_000, dueAtMs: 0, effect: "deny", reason, stopsTurn: false },
      { waitMs: 30_000, dueAtMs: 1.5, effect: "deny", reason, stopsTurn: false },
      {
        waitMs: 30_000,
        dueAtMs: deadline.dueAtMs,
        effect: "later",
        reason,
        stopsTurn: false,
      },
      {
        waitMs: 30_000,
        dueAtMs: deadline.dueAtMs,
        effect: "deny",
        reason: " ",
        stopsTurn: false,
      },
      {
        waitMs: 30_000,
        dueAtMs: deadline.dueAtMs,
        effect: "deny",
        reason: "line\nbreak",
        stopsTurn: false,
      },
      { waitMs: 30_000, dueAtMs: deadline.dueAtMs, effect: "deny", stopsTurn: false },
      { waitMs: 30_000, effect: "deny", reason, stopsTurn: false },
      {
        waitMs: 30_000,
        dueAtMs: deadline.dueAtMs,
        effect: "deny",
        reason,
        stopsTurn: "no",
      },
      { ...deadline, effect: "allow", stopsTurn: true },
      { ...deadline, later: true },
    ])
      expect(() => conversationPermission(withDeadline(broken))).toThrow()
  })

  it("reads the gateway's time the view was sent at, and refuses a view without a usable one", () => {
    expect(conversationView(view(), "conversation").serverNowMs).toBe(1_700_000_000_000)
    for (const broken of [undefined, 0, -1, 1.5, "1700000000000", 2 ** 53]) {
      const value: Record<string, unknown> = view()
      if (broken === undefined) delete value.serverNowMs
      else value.serverNowMs = broken
      expect(() => conversationView(value, "conversation"), String(broken)).toThrow(
        "Invalid conversation serverNowMs",
      )
    }
  })

  it("refuses an app's review that names a tool other than the one it reviews", () => {
    const withTools = (tool: string, toolName: string) => {
      return {
        review: "0".repeat(32),
        executionId: "running",
        permissionId: "permission",
        toolId: "tool",
        title: "Review",
        toolName,
        argumentsJson: "{}",
        origin: { kind: "app", server: "charts", tool },
        ask: "tool",
        options: [{ id: "allow", label: "Allow", effect: "allow" }],
      }
    }
    expect(() =>
      conversationPermission(withTools("app_delete_rows", "app_delete_rows")),
    ).not.toThrow()
    for (const [tool, toolName] of [
      ["app_delete_row", "app_delete_rows"],
      ["App_Delete_Rows", "app_delete_rows"],
    ])
      expect(() => conversationPermission(withTools(tool, toolName))).toThrow(
        "An app's review names a tool other than the one it reviews",
      )
  })

  it("D19 (#390): reads what a review asks as a closed set, and refuses a view whose review asks nothing it knows", () => {
    const withAsk = (ask: unknown, keep = true) => {
      return {
        review: "0".repeat(32),
        executionId: "running",
        permissionId: "permission",
        toolId: "tool",
        title: "The show app on charts asks to send a message as you",
        toolName: "show",
        argumentsJson: '{"text":"Plot May"}',
        origin: { kind: "app", server: "charts", tool: "show" },
        ...(keep ? { ask } : {}),
        options: [{ id: "allow", label: "Allow", effect: "allow" }],
      }
    }
    for (const ask of ["tool", "message"])
      expect(conversationPermission(withAsk(ask)).ask).toBe(ask)
    expect(() => conversationPermission(withAsk(undefined, false))).toThrow()
    for (const ask of [null, "", "Message", "send_message", "tool ", 1, {}])
      expect(() => conversationPermission(withAsk(ask))).toThrow()
  })

  it("D19 (#390): refuses a review the agent asked for that asks to send a message, a contradiction", () => {
    const withOrigin = (ask: string) => {
      return {
        review: "0".repeat(32),
        executionId: "running",
        permissionId: "permission",
        toolId: "tool",
        title: "Run write_file",
        toolName: "write_file",
        argumentsJson: "{}",
        origin: { kind: "harness" },
        ask,
        options: [{ id: "allow", label: "Allow", effect: "allow" }],
      }
    }
    expect(conversationPermission(withOrigin("tool")).ask).toBe("tool")
    expect(() => conversationPermission(withOrigin("message"))).toThrow(
      "A review the agent asked for asks to send a message",
    )
  })

  it("reads what each permission option decides, and refuses an option that does not say", () => {
    const withOptions = (options: unknown[]) => {
      return {
        review: "0".repeat(32),
        executionId: "running",
        permissionId: "permission",
        toolId: "tool",
        title: "Review",
        toolName: "write_file",
        argumentsJson: "{}",
        origin: { kind: "harness" },
        ask: "tool",
        options,
      }
    }
    const read = conversationPermission(
      withOptions([
        { id: "a", label: "Allow", effect: "allow" },
        { id: "d", label: "Deny", effect: "deny" },
      ]),
    )
    expect(read.options.map((option) => option.effect)).toEqual(["allow", "deny"])
    for (const option of [
      { id: "a", label: "Allow" },
      { id: "a", label: "Allow", effect: "always" },
      { id: "a", label: "Allow", effect: "" },
      { id: "a", label: "Allow", effect: true },
    ])
      expect(() => conversationPermission(withOptions([option]))).toThrow()
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
      permissions: [{ ...entry(), extra: true }],
    })
    expect(() => conversationView(permission, "conversation")).toThrow("unknown fields")
    expect(() =>
      conversationPermission({
        review: "0".repeat(32),
        executionId: "running",
        permissionId: "permission",
        toolId: "tool",
        title: "Review",
        toolName: "write_file",
        argumentsJson: "{}",
        origin: { kind: "harness" },
        ask: "tool",
        options: [{ id: "allow", label: "Allow", effect: "allow", extra: true }],
      }),
    ).toThrow("unknown fields")
  })

  it("lists each waiting review by its key alone, whatever is confirmed of the transcript", () => {
    const listing = (permissions: unknown[], transcriptState = "complete") => {
      const value: Record<string, unknown> = view()
      value.permissions = permissions
      if (transcriptState !== "complete") {
        value.transcriptState = transcriptState
        value.queueComplete = false
        value.pending = []
        const capabilities = value.capabilities as Record<string, unknown>
        capabilities.queue = false
        capabilities.steer = false
        capabilities.permissions = false
      }
      return value
    }
    // The most a view lists, each at its largest, in any transcript state.
    const most = Array.from({ length: bounds.maxListedReviews }, (_, index) =>
      entry(index, index % 9 === 0 ? "app" : "harness", "\u{1F600}".repeat(16)),
    )
    expect(conversationView(listing(most), "conversation").permissions).toEqual(most)
    expect(() =>
      conversationView(listing([entry()], "partial"), "conversation"),
    ).not.toThrow()
    for (const broken of [
      [...most, entry(bounds.maxListedReviews)],
      [entry(1), entry(1)],
      [{ ...entry(), review: "0123456789ABCDEF0123456789ABCDEF" }],
      [{ ...entry(), review: "0123456789abcdef" }],
      [{ ...entry(), origin: "plugin" }],
      [{ ...entry(), toolName: "x".repeat(bounds.maxReviewEntryToolNameBytes + 1) }],
      [{ review: entry().review, origin: "harness" }],
    ])
      expect(() => conversationView(listing(broken), "conversation")).toThrow()
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
  const independent = {
    ...refused,
    messages: [
      {
        ...refused.messages[0],
        requiredWorkFailed: true,
        error: "The turn could not complete all required work.",
      },
    ],
  }
  expect(
    conversationView(independent, "conversation").messages[0].requiredWorkFailed,
  ).toBe(true)
  expect(() =>
    conversationView(
      {
        ...independent,
        messages: [{ ...independent.messages[0], requiredWorkFailed: "true" }],
      },
      "conversation",
    ),
  ).toThrow(/requiredWorkFailed/)
  expect(() =>
    conversationView(
      {
        ...independent,
        messages: [{ ...independent.messages[0], authenticationRequired: undefined }],
      },
      "conversation",
    ),
  ).toThrow(/required-work failure/)
  expect(() =>
    conversationView(
      {
        ...independent,
        messages: [{ ...independent.messages[0], error: undefined }],
      },
      "conversation",
    ),
  ).toThrow(/required-work failure/)
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
        // An SSH or paired environment names its host.
        const host =
          key === "environment" && value === "ssh"
            ? { host: "devbox" }
            : key === "environment" && value === "peer"
              ? { host: `peer:${"ab".repeat(32)}` }
              : {}
        const sample =
          key === "state"
            ? states[value]
            : { ...(carriers[key] ?? live), [key]: value, ...host }
        expect(sample, value).toBeDefined()
        expect(conversationView(withLease(sample), "conversation").lease).toEqual(sample)
      }
    }
  })

  it("holds a lease's host to its SSH or paired environment", () => {
    const there = { ...live, environment: "ssh", host: "me@devbox" }
    expect(conversationView(withLease(there), "conversation").lease).toEqual(there)
    const paired = { ...live, environment: "peer", host: `peer:${"ab".repeat(32)}` }
    expect(conversationView(withLease(paired), "conversation").lease).toEqual(paired)
    for (const contradiction of [
      { ...live, environment: "ssh" },
      { ...live, environment: "ssh", host: "" },
      { ...live, environment: "ssh", host: "d".repeat(254) },
      { ...live, environment: "here", host: "devbox" },
      { ...live, environment: "peer" },
      { ...live, environment: "peer", host: "devbox" },
      { ...live, environment: "peer", host: "peer:XYZ" },
      { ...live, environment: "peer", host: `peer:${"AB".repeat(32)}` },
      { ...live, environment: "peer", host: `peer:${"ab".repeat(31)}` },
      { ...live, environment: "peer", host: `peer:${"ab".repeat(33)}` },
      { ...live, environment: "peer", host: `${"ab".repeat(32)}` },
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

describe("the view's artifacts", () => {
  const artifact = {
    executionId: "running",
    lease: "lease-1",
    name: "report.pdf",
    digest: `sha256:${"0f".repeat(32)}`,
    mimeType: "application/pdf",
    size: 2048,
  }
  const withArtifacts = (artifacts: unknown) => ({ ...view(), artifacts })

  it("accepts the published shape, the newest 64 at most", () => {
    const listed = [
      artifact,
      { ...artifact, executionId: undefined },
      { ...artifact, name: "😀".repeat(63) + "abc", size: 64 * 1024 * 1024 },
    ]
    expect(conversationView(withArtifacts(listed), "conversation").artifacts).toEqual(
      listed,
    )
    expect(
      conversationView(withArtifacts(Array(64).fill(artifact)), "conversation").artifacts,
    ).toHaveLength(64)
  })

  it("refuses a view without its list, or a list past its bound", () => {
    expect(() => conversationView(withArtifacts(undefined), "conversation")).toThrow(
      /conversation artifacts/,
    )
    expect(() =>
      conversationView(withArtifacts(Array(65).fill(artifact)), "conversation"),
    ).toThrow(/conversation artifacts/)
  })

  it("refuses an artifact outside the published shape", () => {
    const refused: [unknown, RegExp][] = [
      ["report.pdf", /Invalid conversation response/],
      [{ ...artifact, path: "/outbox/1" }, /unknown fields/],
      [{ ...artifact, executionId: "" }, /executionId/],
      [{ ...artifact, lease: undefined }, /lease/],
      [{ ...artifact, lease: "l".repeat(129) }, /lease/],
      [{ ...artifact, name: "" }, /name/],
      [{ ...artifact, name: "😀".repeat(63) + "abcd" }, /name/],
      [{ ...artifact, digest: `sha256:${"0F".repeat(32)}` }, /artifact digest/],
      [{ ...artifact, mimeType: "Application/PDF" }, /artifact mimeType/],
      [{ ...artifact, size: 0 }, /artifact size/],
      [{ ...artifact, size: 64 * 1024 * 1024 + 1 }, /artifact size/],
      [{ ...artifact, size: 1.5 }, /artifact size/],
    ]
    for (const [item, error] of refused)
      expect(
        () => conversationView(withArtifacts([item]), "conversation"),
        JSON.stringify(item),
      ).toThrow(error)
  })

  it("knows every field the schema gives an artifact", () => {
    const defs = protocolFile("v1.json").$defs
    const keys = Object.keys(defs.ConversationArtifact.properties).sort()
    expect(keys).toEqual(Object.keys(artifact).sort())
  })
})

describe("a share's policy revision", () => {
  it("is bounded as the schema states, for what a change names and what a share answers", () => {
    const defs = protocolFile("v1.json").$defs
    for (const params of ["ConversationShareParams", "ConversationUnshareParams"])
      expect(defs[params].properties.expectedPolicyRevision.maximum, params).toBe(
        maxPolicyRevision,
      )
    expect(defs.ConversationShareResult.properties.policyRevision.maximum).toBe(
      maxPolicyRevision,
    )
    for (const params of ["ConversationShareParams", "ConversationUnshareParams"])
      expect(defs[params].properties.expectedPolicyRevision.minimum, params).toBe(
        minExpectedPolicyRevision,
      )
    // A share answers, and a grant is listed at, a revision the journal gave, never 0.
    expect(defs.ConversationShareResult.properties.policyRevision.minimum).toBe(
      minPolicyRevision,
    )
    expect(defs.ConversationShare.properties.policyRevision.minimum).toBe(
      minPolicyRevision,
    )
  })
})
