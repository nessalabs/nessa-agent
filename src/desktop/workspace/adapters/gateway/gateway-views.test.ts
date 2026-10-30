import { offersAuthenticationRecovery } from "../../../../provider-authentication/model/recovery"
import {
  deadlinePendingLine,
  type ConversationMessage,
  type ConversationPermission,
  type ConversationView,
  type ConversationTool,
} from "@nessa/client"
import { describe, expect, it } from "vitest"
import { appWidget } from "../../../widgets/app/model/app-ref"
import { composerModels } from "../../../model/composer-options"
import { messageText, shownWhole } from "../../model/transcript"
import { defaultModel } from "../../model/workspace-index"
import {
  approvalId,
  handoffId,
  inputId,
  modelFor,
  noteId,
  publishedAfterId,
  unreadableId,
  unreadableOverflowId,
  publishedBeforeId,
  replyId,
  reviewOf,
  runningModel,
  summaryFrom,
  transcriptFrom,
} from "./gateway-views"
import {
  GATEWAY_NOW_MS,
  note,
  questionKey,
  row,
  shownQuestion,
  view,
  type Unkeyed,
} from "./fake-gateway"

const at = () => 42

/** A runtime whose agent is the only fact these recovery tests vary. */
function signInRuntime(agent: string) {
  return {
    model: "x",
    provider: "x",
    workspace: "/",
    agent,
    modelName: "x",
    contextWindowTokens: 1,
    reasoning: false,
  }
}

const turn = (change: Partial<ConversationMessage> = {}): ConversationMessage => ({
  executionId: "e1",
  userText: "Chart the sales",
  attachments: [],
  files: [],
  status: "completed",
  parts: [],
  ...change,
})

const tool = (change: Partial<ConversationTool> = {}): ConversationTool => ({
  executionId: "e1",
  toolId: "t1",
  title: "Draw a chart",
  kind: "other",
  status: "completed",
  details: "",
  input: "",
  nativeHooks: "off",
  ...change,
})

const toolPart = (toolId: string, offset: number) => ({
  offset,
  kind: "tool" as const,
  text: "",
  toolId,
  noticeId: "",
})
const textPart = (text: string, offset: number, messageId?: string) => ({
  offset,
  kind: "text" as const,
  text,
  toolId: "",
  noticeId: "",
  ...(messageId === undefined ? {} : { messageId }),
})

/** A review's key, as a view lists it. */
const reviewKey = (index: number) => index.toString(16).padStart(32, "0")
/**
 * The transcript of a view whose reviews have each been read whole: the view
 * lists them by key, and `permissions` are what each shows.
 */
function reviewedTranscript(
  change: Omit<Partial<ConversationView>, "permissions"> & {
    permissions: Unkeyed<ConversationPermission>[]
  },
  receivedAtMs = GATEWAY_NOW_MS,
) {
  const entries = change.permissions.map((permission, index) => ({
    review: reviewKey(index),
    origin: permission.origin.kind,
    toolName: permission.toolName,
  }))
  return transcriptFrom(
    view("c", { ...change, permissions: entries }),
    1,
    at,
    receivedAtMs,
    change.permissions.map((permission, index) => ({
      state: "loaded" as const,
      entry: entries[index]!,
      shown: { ...permission, review: entries[index]!.review },
    })),
  )
}

describe("what a bounded view says about history it did not draw", () => {
  const states = [
    "not_loaded",
    "partial",
    "complete_empty",
    "complete",
    "stale",
    "unknown",
  ] as const

  it("carries omitted and unconfirmed history, and nothing when the view says it is whole", () => {
    for (const transcriptState of states) {
      for (const truncated of [false, true]) {
        const history = transcriptFrom(
          view("c", { transcriptState, truncated }),
          1,
          at,
          GATEWAY_NOW_MS,
        ).history
        const unconfirmed =
          transcriptState === "not_loaded" ||
          transcriptState === "partial" ||
          transcriptState === "stale" ||
          transcriptState === "unknown"
        if (!truncated && !unconfirmed) expect(history).toBeUndefined()
        else expect(history).toEqual({ omitted: truncated, unconfirmed })
      }
    }
  })

  it("keeps the turns the view did draw when it says older ones were left out", () => {
    const transcript = transcriptFrom(
      view("c", {
        truncated: true,
        transcriptState: "complete",
        messages: [turn({ userText: "recent", parts: [textPart("still here", 0)] })],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages.map((message) => message.id)).toEqual([
      inputId("e1"),
      replyId("e1"),
    ])
    expect(transcript.history).toEqual({ omitted: true, unconfirmed: false })
  })
})

describe("a conversation view as a transcript", () => {
  it("keeps an MCP App's widget part after its tool's step, and none for a tool without a UI", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [turn({ parts: [toolPart("t1", 0), toolPart("t2", 1)] })],
        tools: [
          tool({
            mcp: { server: "charts", tool: "draw", resourceUri: "ui://charts/draw" },
          }),
          tool({
            toolId: "t2",
            title: "List rows",
            mcp: { server: "charts", tool: "rows" },
          }),
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    const reply = transcript.messages.find((message) => message.role === "agent")
    expect(reply?.parts).toEqual([
      { kind: "step", step: "run", label: "Draw a chart", id: "t1", status: "completed" },
      { kind: "widget", widget: appWidget("charts", "c", "e1", "t1") },
      { kind: "step", step: "run", label: "List rows", id: "t2", status: "completed" },
    ])
  })

  it("names the input by its execution id, so the window's message is found under the id it sent", () => {
    const sent = "0b6a4f0e-2a1c-4f53-9a55-3f0f1a6d2b7c"
    const transcript = transcriptFrom(
      view("c", {
        messages: [turn({ executionId: sent, parts: [textPart("Done", 0)] })],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages.map((message) => message.id)).toEqual([
      sent,
      replyId(sent),
    ])
  })

  it("never gives two messages one id, whatever the execution ids", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({ executionId: "a", parts: [textPart("x", 0)] }),
          turn({ executionId: "a/reply", parts: [textPart("y", 0)] }),
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    const ids = transcript.messages.map((message) => message.id)
    expect(new Set(ids).size).toBe(ids.length)
    expect(ids[0]).toBe(inputId("a"))
  })

  it("joins text the provider names as one message, and nothing else; thoughts are not shown", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({
            parts: [
              textPart("Hel", 0, "m1"),
              textPart("lo", 1, "m1"),
              { ...textPart("hmm", 2), kind: "thought" },
              textPart("Next", 3, "m2"),
              textPart(" one", 4),
              textPart(" two", 5),
              { ...textPart("Declined", 6), kind: "local_notice", noticeId: "n" },
            ],
          }),
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages[1].parts).toEqual([
      { kind: "text", text: "Hello" },
      { kind: "text", text: "Next" },
      { kind: "text", text: " one" },
      { kind: "text", text: " two" },
      { kind: "text", text: "Declined" },
    ])
  })

  it("reads a tool's kind into the window's four, the rest as run", () => {
    const kinds = [
      "read",
      "edit",
      "delete",
      "move",
      "search",
      "fetch",
      "execute",
      "think",
      "",
    ] as const
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({ parts: kinds.map((_, index) => toolPart(`t${index}`, index)) }),
        ],
        tools: kinds.map((kind, index) => tool({ toolId: `t${index}`, kind })),
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(
      transcript.messages[1].parts.map((part) =>
        part.kind === "step" ? part.step : null,
      ),
    ).toEqual(["read", "edit", "edit", "edit", "search", "search", "run", "run", "run"])
  })

  it("lets go a tool part the bounded view has no tool for", () => {
    const transcript = transcriptFrom(
      view("c", { messages: [turn({ parts: [toolPart("gone", 0)] })] }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages).toHaveLength(1)
  })

  it("shows inputs still waiting after the turns, once each", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [turn({ executionId: "e1", status: "queued" })],
        pending: [
          {
            executionId: "e1",
            text: "Chart the sales",
            attachments: [],
            files: [],
            mode: "queued",
          },
          {
            executionId: "e2",
            text: "And costs",
            attachments: [],
            files: [],
            mode: "queued",
          },
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(
      transcript.messages.map((message) => [message.id, messageText(message)]),
    ).toEqual([
      ["e1", "Chart the sales"],
      ["e2", "And costs"],
    ])
  })

  it("ends a turn's reply with the files it published, and puts the rest after the last turn", () => {
    const file = (name: string, executionId?: string) => ({
      ...(executionId === undefined ? {} : { executionId }),
      lease: "lease-1",
      name,
      digest: `sha256:${"a".repeat(64)}`,
      mimeType: "image/png",
      size: 3,
    })
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({ executionId: "e1", parts: [textPart("Charted", 0)] }),
          // A turn that said nothing but published a file still has a reply to hold it.
          turn({ executionId: "e2", userText: "Again" }),
        ],
        artifacts: [
          file("chart.png", "e1"),
          file("loose.png"),
          file("again.png", "e2"),
          file("older.png", "e0"),
        ],
        pending: [
          {
            executionId: "e3",
            text: "Waiting",
            attachments: [],
            files: [],
            mode: "queued",
          },
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    const shown = transcript.messages.map((message) => [
      message.id,
      message.parts.map((part) =>
        part.kind === "file" ? `file:${part.name}` : part.kind,
      ),
    ])
    expect(shown).toEqual([
      // A turn the bounded view left out is older than every turn shown.
      [publishedBeforeId, ["file:older.png"]],
      [inputId("e1"), ["text"]],
      [replyId("e1"), ["text", "file:chart.png"]],
      [inputId("e2"), ["text"]],
      [replyId("e2"), ["file:again.png"]],
      [publishedAfterId, ["file:loose.png"]],
      [inputId("e3"), ["text"]],
    ])
    // Named exactly as the conversation holds it, and nothing of the lease.
    expect(transcript.messages[2]!.parts[1]).toEqual({
      kind: "file",
      name: "chart.png",
      digest: `sha256:${"a".repeat(64)}`,
      mimeType: "image/png",
      size: 3,
    })
    // No other message ever has the loose files' ids.
    expect(new Set(transcript.messages.map((message) => message.id)).size).toBe(7)
  })

  it("shows each note as a row of its own right after the turn or waiting input it followed, and one left before any turn or after a dropped turn first", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({ executionId: "e1", parts: [textPart("Charted", 0)] }),
          turn({ executionId: "e2", userText: "Again" }),
        ],
        artifacts: [
          {
            executionId: "e1",
            lease: "lease-1",
            name: "chart.png",
            digest: `sha256:${"a".repeat(64)}`,
            mimeType: "image/png",
            size: 3,
          },
        ],
        notes: [
          note("first"),
          // Its turn is older than every turn the bounded view kept.
          note("dropped", { afterExecutionId: "e0" }),
          note("after-e1", { afterExecutionId: "e1" }),
          note("unlisted", { afterExecutionId: "e3" }),
          note("after-e2", {
            afterExecutionId: "e2",
            delivery: "next_turn",
            deliveryState: "retired",
            retiredReason: "expired",
            author: { kind: "grant", surface: "phone" },
          }),
        ],
        pending: [
          {
            executionId: "e3",
            text: "Waiting",
            attachments: [],
            files: [],
            mode: "queued",
          },
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages.map((message) => [message.id, message.role])).toEqual([
      [noteId("first"), "note"],
      [noteId("dropped"), "note"],
      [inputId("e1"), "user"],
      // After the whole turn: its reply and the files it published.
      [replyId("e1"), "agent"],
      [noteId("after-e1"), "note"],
      [inputId("e2"), "user"],
      [noteId("after-e2"), "note"],
      [inputId("e3"), "user"],
      // Right after the input still waiting it followed.
      [noteId("unlisted"), "note"],
    ])
    const shown = transcript.messages.find((message) => message.id === noteId("after-e2"))
    // What its row draws, and nothing of the gateway's own record.
    expect(shown?.note).toEqual({
      id: "after-e2",
      text: "Note after-e2",
      author: { kind: "grant", surface: "phone" },
      deliveryState: "retired",
      retiredReason: "expired",
      afterExecutionId: "e2",
    })
    expect(shown?.parts).toEqual([])
    // A note is no turn's text: nothing reads it as what was said.
    expect(messageText(shown!)).toBe("")
    expect(new Set(transcript.messages.map((message) => message.id)).size).toBe(9)
  })

  it("shows a record it could not read as its own line after the turn it followed, and one with no turn first", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [turn({ executionId: "e1", parts: [textPart("Charted", 0)] })],
        unreadable: [{ id: "3" }, { id: "9", afterExecutionId: "e1" }],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages.map((message) => [message.id, message.role])).toEqual([
      [unreadableId("3"), "unreadable"],
      [inputId("e1"), "user"],
      [replyId("e1"), "agent"],
      [unreadableId("9"), "unreadable"],
    ])
    const line = transcript.messages.find((message) => message.role === "unreadable")
    expect(line && "unreadable" in line ? line.unreadable : null).toEqual({ id: "3" })
    expect(messageText(transcript.messages[0]!)).toBe("")
    expect(new Set(transcript.messages.map((message) => message.id)).size).toBe(4)
  })

  it("draws a switch as a line after the turn it followed, and a fork's copied turns first, then its line", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({ executionId: "e1", parts: [textPart("Charted", 0)] }),
          turn({ executionId: "e2", parts: [textPart("Sailed", 0)] }),
        ],
        notes: [note("n1", { afterExecutionId: "e1" })],
        handoffs: [
          {
            handoffId: "fork",
            kind: "fork",
            atMs: 1,
            turns: [
              {
                said: "Plan a trip",
                reply: "Lisbon",
                agent: { conversationId: "source-chat", executionId: "source-turn" },
              },
              { reply: "Only a reply" },
            ],
            omittedTurns: 2,
          },
          {
            handoffId: "switch",
            kind: "switch",
            atMs: 2,
            afterExecutionId: "e1",
            from: { agent: "claude", model: "claude-opus-5" },
            to: { agent: "codex", model: "gpt-6-astra" },
          },
          // After a turn the bounded view left out: older than every turn shown.
          {
            handoffId: "dropped",
            kind: "switch",
            atMs: 0,
            afterExecutionId: "e0",
            from: { model: "old" },
            to: { agent: "claude", model: "claude-opus-5" },
          },
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages[0]).toMatchObject({
      role: "user",
      agent: { conversationId: "source-chat", executionId: "source-turn" },
    })
    expect(transcript.messages.map((message) => [message.id, message.role])).toEqual([
      [`${handoffId("fork")}/0`, "user"],
      [`${handoffId("fork")}/0/reply`, "agent"],
      [`${handoffId("fork")}/1/reply`, "agent"],
      [handoffId("fork"), "handoff"],
      [handoffId("dropped"), "handoff"],
      [inputId("e1"), "user"],
      [replyId("e1"), "agent"],
      [noteId("n1"), "note"],
      [handoffId("switch"), "handoff"],
      [inputId("e2"), "user"],
      [replyId("e2"), "agent"],
    ])
    const lines = transcript.messages.flatMap((message) =>
      message.role === "handoff" ? [message.handoff] : [],
    )
    expect(lines).toEqual([
      { kind: "fork", omittedTurns: 2 },
      {
        kind: "switch",
        from: { model: "old" },
        to: { agent: "claude", model: "claude-opus-5" },
      },
      {
        kind: "switch",
        from: { agent: "claude", model: "claude-opus-5" },
        to: { agent: "codex", model: "gpt-6-astra" },
      },
    ])
    expect(messageText(transcript.messages[0]!)).toBe("Plan a trip")
    expect(new Set(transcript.messages.map((message) => message.id)).size).toBe(11)
  })

  it("ends with one line when more records could not be read than the view lists", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [turn({ executionId: "e1", parts: [textPart("Charted", 0)] })],
        unreadable: [{ id: "3" }],
        unreadableTruncated: true,
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages.at(-1)).toEqual({
      id: unreadableOverflowId,
      role: "unreadable-overflow",
      at: 42,
      parts: [],
    })
    expect(messageText(transcript.messages.at(-1)!)).toBe("")
  })

  it("puts a note left between two inputs waiting right after the first, as the gateway lists them", () => {
    const queued = (executionId: string, text: string) => ({
      executionId,
      text,
      attachments: [],
      files: [],
      mode: "queued" as const,
    })
    // The gateway lists every input waiting both as a queued turn and as pending.
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({ executionId: "e1", parts: [textPart("Done", 0)] }),
          turn({ executionId: "q1", userText: "First waiting", status: "queued" }),
          turn({ executionId: "q2", userText: "Second waiting", status: "queued" }),
        ],
        pending: [queued("q1", "First waiting"), queued("q2", "Second waiting")],
        notes: [
          note("between", { afterExecutionId: "q1" }),
          note("after-q2", { afterExecutionId: "q2" }),
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages.map((message) => message.id)).toEqual([
      inputId("e1"),
      replyId("e1"),
      inputId("q1"),
      noteId("between"),
      inputId("q2"),
      noteId("after-q2"),
    ])
  })

  it("draws an input waiting that the view lists apart from its turns after every turn, its note right after it, as the panel does", () => {
    const queued = (executionId: string, text: string) => ({
      executionId,
      text,
      attachments: [],
      files: [],
      mode: "queued" as const,
    })
    // A bounded view can list a waiting input without its turn. Which order
    // waiting inputs are drawn in is the panel's and the desktop's one shared
    // answer: the listed turns first, then the rest (`waiting-notes.test.tsx`).
    const transcript = transcriptFrom(
      view("c", {
        truncated: true,
        messages: [
          turn({ executionId: "e1", parts: [textPart("Done", 0)] }),
          turn({ executionId: "q1", userText: "Listed waiting", status: "queued" }),
        ],
        pending: [queued("p0", "Unlisted waiting"), queued("q1", "Listed waiting")],
        notes: [note("after-p0", { afterExecutionId: "p0" })],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages.map((message) => message.id)).toEqual([
      inputId("e1"),
      replyId("e1"),
      inputId("q1"),
      inputId("p0"),
      noteId("after-p0"),
    ])
    // The latest input is still the last one the view lists apart, as before.
    expect(transcript.latestInputId).toBe("p0")
  })

  it("names a note apart from every input, reply and list of published files", () => {
    for (const messageId of ["reply", "published", "a/b", "/note:x", ""])
      expect(noteId(messageId)).toMatch(/^\/note:[^/]*$/)
    expect(noteId("a/b")).not.toBe(noteId("a%002Fb"))
    expect([
      inputId("x"),
      replyId("x"),
      publishedAfterId,
      publishedBeforeId,
    ]).not.toContain(noteId("x"))
  })

  it("D17 (#390): says which app wrote a turn, waiting or not, and nothing of the person's own", () => {
    const app = { executionId: "e0", toolId: "t0", server: "charts", tool: "show" }
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({ executionId: "e1", app }),
          turn({ executionId: "e2", userText: "mine" }),
        ],
        pending: [
          {
            executionId: "e3",
            text: "Plot May",
            attachments: [],
            files: [],
            app,
            mode: "queued",
          },
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    const users = transcript.messages.filter((message) => message.role === "user")
    expect(users.map((message) => [message.parts, message.app])).toEqual([
      [[{ kind: "text", text: "Chart the sales" }], { server: "charts", tool: "show" }],
      [[{ kind: "text", text: "mine" }], undefined],
      [[{ kind: "text", text: "Plot May" }], { server: "charts", tool: "show" }],
    ])
    // Waiting, it has its label, and no delivery state of its own.
    expect(users[2]).not.toHaveProperty("delivery")
  })

  it("says what a running turn does until its reply streams, then nothing", () => {
    const working = transcriptFrom(
      view("c", {
        messages: [turn({ status: "running", parts: [toolPart("t1", 0)] })],
        tools: [tool({ title: "Reading files" })],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(working.activity).toEqual({ label: "Reading files", since: 42 })
    const thinking = transcriptFrom(
      view("c", { messages: [turn({ status: "running" })] }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(thinking.activity?.label).toBe("Thinking")
    const streaming = transcriptFrom(
      view("c", { messages: [turn({ status: "running", parts: [textPart("Hi", 0)] })] }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(streaming.activity).toBeNull()
    expect(
      transcriptFrom(view("c", { messages: [turn()] }), 1, at, GATEWAY_NOW_MS).activity,
    ).toBeNull()
  })

  it("marks a reply working while its turn runs or waits, and settled once it ended however it ended", () => {
    const reply = (status: ConversationMessage["status"]) =>
      transcriptFrom(
        view("c", {
          messages: [turn({ status, parts: [toolPart("t1", 0)] })],
          tools: [tool({ title: "Reading files" })],
        }),
        1,
        at,
        GATEWAY_NOW_MS,
      ).messages.find((message) => message.role === "agent")
    expect(reply("running")).toMatchObject({ working: true })
    expect(reply("queued")).toMatchObject({ working: true })
    for (const ended of ["completed", "failed", "cancelled", "unresolved"] as const)
      expect(reply(ended), ended).not.toHaveProperty("working")
  })

  it("lists a review it has not read yet as loading, and offers no answer on it until it is read", () => {
    const entry = { review: reviewKey(0), origin: "harness" as const, toolName: "Write" }
    const listed = view("c", {
      messages: [turn({ status: "running" })],
      permissions: [entry],
    })
    for (const [details, loading] of [
      [[], "reading"],
      [[{ state: "loading", entry }], "reading"],
      [[{ state: "failed", entry, error: new Error("unavailable") }], "failed"],
    ] as const) {
      const approval = transcriptFrom(listed, 1, at, GATEWAY_NOW_MS, details)
        .approvals[0]!
      expect(approval).toMatchObject({
        id: reviewKey(0),
        command: "Write",
        loading,
        options: [],
      })
      expect(shownWhole(approval)).toBe(false)
    }
  })

  it("projects every review the view lists, in its order, each loading on its own", () => {
    const entries = [0, 1, 2].map((index) => ({
      review: reviewKey(index),
      origin: "harness" as const,
      toolName: `Tool${index}`,
    }))
    const listed = view("c", {
      messages: [turn({ status: "running" })],
      permissions: entries,
    })
    const failed = {
      state: "failed",
      entry: entries[0],
      error: new Error("unavailable"),
    } as const
    const { approvals } = transcriptFrom(listed, 1, at, GATEWAY_NOW_MS, [failed])
    expect(
      approvals.map((approval) => [approval.id, approval.command, approval.loading]),
    ).toEqual([
      [reviewKey(0), "Tool0", "failed"],
      [reviewKey(1), "Tool1", "reading"],
      [reviewKey(2), "Tool2", "reading"],
    ])
  })

  it("asks a pending permission as an approval, under an id that names the review", () => {
    const transcript = reviewedTranscript({
      messages: [turn({ status: "running" })],
      permissions: [
        {
          executionId: "e/1",
          permissionId: "p%2",
          toolId: "t1",
          title: "Tool permission",
          options: [{ id: "a", label: "Allow", effect: "allow" }],
          toolName: "bash",
          origin: { kind: "harness" },
          ask: "tool",
          argumentsJson: '{"command":"rm -rf build"}',
        },
      ],
    })
    expect(transcript.approvals[0]).toMatchObject({
      // The exact input approved, whatever the provider titles the review.
      command: 'bash {"command":"rm -rf build"}',
      reason: "Tool permission",
      origin: { kind: "agent" },
      // The review's own answers, and no always: the view offered none.
      options: [{ id: "a", label: "Allow", choice: "once" }],
      ask: "tool",
    })
    expect(transcript.approvals[0]?.deadline).toBeUndefined()
    const waiting = reviewedTranscript({
      messages: [turn({ status: "running" })],
      permissions: [
        {
          executionId: "e/1",
          permissionId: "p%2",
          toolId: "t1",
          title: "Tool permission",
          options: [{ id: "a", label: "Allow", effect: "allow" }],
          toolName: "bash",
          origin: { kind: "harness" },
          ask: "tool",
          argumentsJson: '{"command":"rm -rf build"}',
          deadline: {
            waitMs: 30_000,
            dueAtMs: 1_700_000_030_000,
            effect: "deny",
            reason:
              "No human answered in time. Continue without this tool or try another approach.",
            stopsTurn: true,
          },
        },
      ],
    })
    expect(waiting.approvals[0]?.deadline).toEqual({
      waitMs: 30_000,
      dueAtMs: 1_700_000_030_000,
      effect: "deny",
      reason:
        "No human answered in time. Continue without this tool or try another approach.",
      stopsTurn: true,
    })
    // The approval is the review's key, whatever its ids: they can be used again.
    expect(reviewOf(transcript.approvals[0]!.id)).toBe(reviewKey(0))
  })

  it("puts the deadline on the window's clock, so a clock ahead or behind the gateway's counts down the same", () => {
    // Due 30 s after the gateway sent the view, on the gateway's clock.
    const asked = (skewMs: number) => {
      const receivedAtMs = GATEWAY_NOW_MS + skewMs
      const transcript = reviewedTranscript(
        {
          permissions: [
            {
              executionId: "e1",
              permissionId: "p1",
              toolId: "t1",
              title: "Tool permission",
              options: [{ id: "d", label: "Deny", effect: "deny" }],
              toolName: "bash",
              origin: { kind: "harness" },
              ask: "tool",
              argumentsJson: "{}",
              deadline: {
                waitMs: 30_000,
                dueAtMs: GATEWAY_NOW_MS + 30_000,
                effect: "deny",
                reason: "No human answered in time.",
                stopsTurn: false,
              },
            },
          ],
        },
        receivedAtMs,
      )
      return { deadline: transcript.approvals[0]!.deadline!, receivedAtMs }
    }
    for (const skewMs of [0, 60_000, -60_000]) {
      const { deadline, receivedAtMs } = asked(skewMs)
      expect(deadline.dueAtMs - receivedAtMs, `skew ${skewMs}`).toBe(30_000)
      expect(deadlinePendingLine(deadline, receivedAtMs)).toBe(
        "If nobody answers within 30 seconds, this is denied automatically.",
      )
    }
  })
})

describe("who asks for an approval (#436)", () => {
  const asked = (
    origin: ConversationPermission["origin"],
    ask: ConversationPermission["ask"] = "tool",
  ) =>
    reviewedTranscript({
      messages: [turn({ status: "completed" })],
      permissions: [
        {
          executionId: "e1",
          permissionId: "app-1",
          toolId: "t1",
          title: "An app asks to run app_delete_row on mcptest",
          options: [{ id: "allow", label: "Allow", effect: "allow" }],
          toolName: "app_delete_row",
          origin,
          ask,
          argumentsJson: "{}",
        },
      ],
    }).approvals[0]

  it("carries each offered answer in the view's order and words, and none it did not offer (#444)", () => {
    const approval = asked({ kind: "harness" })
    // The fixture lists only allow. A deny the view does not carry is not invented,
    // and neither is always.
    expect(approval?.options).toEqual([{ id: "allow", label: "Allow", choice: "once" }])
    const remembered = reviewedTranscript({
      messages: [turn({ status: "completed" })],
      permissions: [
        {
          executionId: "e1",
          permissionId: "p",
          toolId: "t1",
          title: "Run it",
          options: [
            { id: "allow-once", label: "Allow", effect: "allow" },
            { id: "allow_always", label: "Always allow", effect: "allow" },
            {
              id: "nessa.remember.allow",
              label: "Always allow in this project",
              effect: "allow",
            },
            {
              id: "nessa.remember.deny",
              label: "Always deny in this project",
              effect: "deny",
            },
          ],
          toolName: "bash",
          origin: { kind: "harness" },
          ask: "tool",
          argumentsJson: "{}",
        },
      ],
    }).approvals[0]
    expect(remembered?.options).toEqual([
      { id: "allow-once", label: "Allow", choice: "once" },
      { id: "allow_always", label: "Always allow", choice: "once" },
      {
        id: "nessa.remember.allow",
        label: "Always allow in this project",
        choice: "always",
      },
      { id: "nessa.remember.deny", label: "Always deny in this project", choice: "deny" },
    ])
    const both = reviewedTranscript({
      messages: [turn({ status: "completed" })],
      permissions: [
        {
          executionId: "e1",
          permissionId: "p",
          toolId: "t1",
          title: "Run it",
          options: [
            { id: "opt-b", label: "Nope", effect: "deny" },
            { id: "opt-a", label: "Sure", effect: "allow" },
          ],
          toolName: "bash",
          origin: { kind: "harness" },
          ask: "tool",
          argumentsJson: "{}",
        },
      ],
    }).approvals[0]
    expect(both?.options).toEqual([
      { id: "opt-b", label: "Nope", choice: "deny" },
      { id: "opt-a", label: "Sure", choice: "once" },
    ])
  })

  it("O1: a review the agent's harness asked for is the agent's", () => {
    expect(asked({ kind: "harness" })?.origin).toEqual({ kind: "agent" })
  })

  it("O2: a review an app asked for is the app's, naming its server and the tool", () => {
    expect(
      asked({ kind: "app", server: "mcptest", tool: "app_delete_row" })?.origin,
    ).toEqual({ kind: "app", server: "mcptest", tool: "app_delete_row" })
  })

  it("D19 (#390): carries what the review asks as the gateway says it", () => {
    const app = { kind: "app", server: "mcptest", tool: "show_rows" } as const
    expect(asked(app, "message")?.ask).toBe("message")
    expect(asked(app, "tool")?.ask).toBe("tool")
    expect(asked({ kind: "harness" }, "tool")?.ask).toBe("tool")
  })
})

describe("approval ids", () => {
  it("read back only what they wrote", () => {
    const id = approvalId({ review: reviewKey(7), origin: "harness", toolName: "bash" })
    expect(reviewOf(id)).toBe(reviewKey(7))
    for (const foreign of [
      "",
      "a",
      "a/b/c",
      "a%/b",
      "%zz/b",
      reviewKey(0xab).toUpperCase(),
    ])
      expect(reviewOf(foreign)).toBeNull()
  })
})

describe("a list row as a summary", () => {
  it("is running, waiting on the person, or idle", () => {
    const model = { provider: "anthropic", modelId: "claude-opus-5" }
    expect(
      summaryFrom(row("a", { running: true }), { model, waitingOnPerson: false }).status,
    ).toBe("running")
    expect(
      summaryFrom(row("a", { running: true }), { model, waitingOnPerson: true }).status,
    ).toBe("needs-you")
    expect(summaryFrom(row("a"), { model, waitingOnPerson: false }).status).toBe("idle")
  })
})

describe("the model a session runs on", () => {
  it("is the catalogue's entry for what the gateway runs, or none it does not list", () => {
    // A model the catalogue lists, whichever it is: the catalogue is the SDK's.
    const listed = composerModels[composerModels.length - 1]
    const runs = (model: string) =>
      view("c", {
        model,
        runtime: {
          model: "runtime-model-not-the-selection",
          provider: "codex",
          workspace: "/",
          agent: "codex",
          modelName: "Some model",
          contextWindowTokens: 1,
          reasoning: true,
        },
      })
    expect(runningModel(runs(listed.modelId))).toEqual({
      provider: listed.provider,
      modelId: listed.modelId,
    })
    expect(runningModel(runs("a-model-no-catalogue-lists"))).toBeUndefined()
    expect(runningModel(view("c", { runtime: undefined }))).toBeUndefined()
    expect(
      runningModel(view("c", { model: listed.modelId, runtime: undefined })),
    ).toEqual({
      provider: listed.provider,
      modelId: listed.modelId,
    })
  })

  it("is said as the known one, else the composer's default", () => {
    const known = { provider: "openai", modelId: "gpt-5" }
    expect(modelFor(known)).toBe(known)
    expect(modelFor(undefined)).toEqual(defaultModel())
  })
})

describe("usage limits", () => {
  it("compares the reset on the gateway timeline", () => {
    const reset = GATEWAY_NOW_MS + 30_000
    const receivedAt = GATEWAY_NOW_MS + 120_000
    const transcript = transcriptFrom(
      view("limit", {
        usageLimitAutoContinue: true,
        messages: [
          turn({
            status: "failed",
            usageLimit: {
              provider: "claude",
              recovery: "timed_reset",
              resetsAtMs: reset,
            },
          }),
        ],
      }),
      1,
      at,
      receivedAt,
    )
    expect(transcript.messages[0]).toMatchObject({
      usageLimit: {
        resetsAtMs: reset,
        resetsAtClientMs: receivedAt + 30_000,
      },
    })
    const past = GATEWAY_NOW_MS - 10_000
    const slow = GATEWAY_NOW_MS - 120_000
    const later = transcriptFrom(
      view("later", {
        messages: [
          turn({
            status: "failed",
            usageLimit: {
              provider: "claude",
              recovery: "timed_reset",
              resetsAtMs: past,
            },
          }),
        ],
      }),
      1,
      at,
      slow,
    )
    expect(later.messages[0]).toMatchObject({
      usageLimit: { resetsAtMs: past, resetsAtClientMs: slow - 10_000 },
    })
  })

  it("keeps the classification and the gateway preference, without a diagnostic", () => {
    const source = view("limit", {
      usageLimitAutoContinue: false,
      messages: [
        turn({
          status: "failed",
          usageLimit: {
            provider: "codex",
            recovery: "unknown",
          },
        }),
      ],
    })
    const transcript = transcriptFrom(source, 1, at, GATEWAY_NOW_MS)
    expect(transcript.usageLimitAutoContinue).toBe(false)
    expect(transcript.messages[0]).toMatchObject({
      role: "user",
      executionId: "e1",
      usageLimit: { provider: "codex", recovery: "unknown" },
      terminal: { status: "failed" },
    })
    expect(transcript.messages[0]).not.toHaveProperty("requiredWorkFailed")
    const marked = transcript.messages[0]
    if (marked && "terminal" in marked)
      expect(marked.terminal).not.toHaveProperty("error")
  })
})

describe("provider authentication recovery", () => {
  it("uses only the latest turn's typed refusal, never its diagnostic text", () => {
    const source = view("auth")
    source.messages = [turn({ status: "failed", authenticationRequired: true })]
    expect(recovery(transcriptFrom(source, 1, at, GATEWAY_NOW_MS))).toBe(true)
    source.messages.push(turn({ executionId: "later" }))
    expect(recovery(transcriptFrom(source, 2, at, GATEWAY_NOW_MS))).toBe(false)
    source.messages = [turn({ status: "failed", error: "OAuth session expired" })]
    expect(recovery(transcriptFrom(source, 3, at, GATEWAY_NOW_MS))).toBe(false)
  })
})

it("shows auth recovery without duplicating its diagnostic and preserves unrelated notices", () => {
  const source = view("auth", {
    runtime: signInRuntime("claude"),
    messages: [
      turn({
        status: "failed",
        authenticationRequired: true,
        error: "Internal error: OAuth session expired",
        parts: [
          {
            ...textPart("Nessa declined a tool review.", 0),
            kind: "local_notice",
            noticeId: "review-1",
          },
        ],
      }),
    ],
  })
  const transcript = transcriptFrom(source, 1, at, GATEWAY_NOW_MS)
  expect(recovery(transcript)).toBe(true)
  expect(transcript.messages.flatMap((message) => message.parts)).toEqual([
    { kind: "text", text: "Chart the sales" },
    { kind: "text", text: "Nessa declined a tool review." },
  ])
  expect(transcript.messages[1]).toMatchObject({
    role: "agent",
    executionId: "e1",
    terminal: { status: "failed", error: "Internal error: OAuth session expired" },
  })
  expect(transcript.messages[1]).not.toHaveProperty("requiredWorkFailed")
})

it("keeps an independent failure beside authentication recovery", () => {
  const transcript = transcriptFrom(
    view("auth", {
      runtime: signInRuntime("claude"),
      messages: [
        turn({
          status: "failed",
          authenticationRequired: true,
          requiredWorkFailed: true,
          error: "The turn could not complete all required work.",
        }),
      ],
    }),
    1,
    at,
    GATEWAY_NOW_MS,
  )
  expect(recovery(transcript)).toBe(true)
  expect(transcript.messages[0]).toMatchObject({
    role: "user",
    executionId: "e1",
    requiredWorkFailed: true,
    terminal: {
      status: "failed",
      error: "The turn could not complete all required work.",
    },
  })
})

it("does not treat the required-work sentence as the independent failure", () => {
  const sentence = "The turn could not complete all required work."
  const transcript = transcriptFrom(
    view("auth", {
      runtime: signInRuntime("codex"),
      messages: [
        turn({
          status: "failed",
          authenticationRequired: true,
          error: sentence,
        }),
      ],
    }),
    1,
    at,
    GATEWAY_NOW_MS,
  )
  expect(recovery(transcript)).toBe(true)
  expect(transcript.messages[0]).toMatchObject({
    terminal: { status: "failed", error: sentence },
  })
  expect(transcript.messages[0]).not.toHaveProperty("requiredWorkFailed")
  const flagged = transcriptFrom(
    view("auth", {
      runtime: signInRuntime("codex"),
      messages: [
        turn({
          status: "failed",
          authenticationRequired: true,
          requiredWorkFailed: true,
          error: "storage write failed",
        }),
      ],
    }),
    2,
    at,
    GATEWAY_NOW_MS,
  )
  expect(flagged.messages[0]).toMatchObject({
    requiredWorkFailed: true,
    terminal: { status: "failed", error: "storage write failed" },
  })
})

it("keeps a Failed mark on an authentication refusal once another turn has followed", () => {
  const refusal = turn({
    status: "failed",
    authenticationRequired: true,
    error: "Internal error: OAuth session expired",
  })
  const followed = transcriptFrom(
    view("auth", {
      runtime: signInRuntime("claude"),
      messages: [
        refusal,
        turn({
          executionId: "later",
          userText: "Try again",
          status: "completed",
          parts: [textPart("Ready", 0)],
        }),
      ],
    }),
    1,
    at,
    GATEWAY_NOW_MS,
  )
  expect(recovery(followed)).toBe(false)
  expect(followed.messages[0]).toMatchObject({
    role: "user",
    terminal: { status: "failed", error: "Internal error: OAuth session expired" },
  })
  const waiting = view("auth", { runtime: signInRuntime("claude"), messages: [refusal] })
  waiting.pending = [
    {
      executionId: "retry",
      text: "Try again",
      attachments: [],
      files: [],
      mode: "queued",
    },
  ]
  const superseded = transcriptFrom(waiting, 2, at, GATEWAY_NOW_MS)
  expect(recovery(superseded)).toBe(false)
  expect(superseded.messages[0]).toMatchObject({
    role: "user",
    terminal: { status: "failed", error: "Internal error: OAuth session expired" },
  })
})

it("keeps an authentication failure when sign-in cannot render", () => {
  const refused = turn({
    status: "failed",
    authenticationRequired: true,
    error: "OAuth session expired",
  })
  const bare = turn({ status: "failed", authenticationRequired: true })
  const shown = (source: ReturnType<typeof view>) =>
    transcriptFrom(source, 1, at, GATEWAY_NOW_MS)
  for (const source of [
    view("auth", { messages: [refused] }),
    view("auth", { runtime: signInRuntime("opencode"), messages: [refused] }),
    view("auth", { runtime: signInRuntime("other"), messages: [refused] }),
    view("auth", { runtime: signInRuntime("Claude"), messages: [refused] }),
    view("auth", { runtime: signInRuntime(""), messages: [bare] }),
  ]) {
    const transcript = shown(source)
    expect(recovery(transcript)).toBe(true)
    expect(transcript.messages[0]).toMatchObject({
      role: "user",
      terminal: source.messages[0]?.error
        ? { status: "failed", error: "OAuth session expired" }
        : { status: "failed" },
    })
    if (!source.messages[0]?.error) {
      expect(transcript.messages[0]).not.toHaveProperty("terminal.error")
    }
  }
  for (const agent of ["claude", "codex"] as const) {
    const transcript = shown(
      view("auth", { runtime: signInRuntime(agent), messages: [refused] }),
    )
    expect(recovery(transcript)).toBe(true)
    expect(transcript.messages[0]).toMatchObject({
      executionId: "e1",
      terminal: { status: "failed", error: "OAuth session expired" },
    })
    expect(transcript.messages[0]).not.toHaveProperty("requiredWorkFailed")
  }
})

it("recovery follows the latest mapped input across pending and dispatched turns", () => {
  const refusal = turn({ status: "failed", authenticationRequired: true })
  const pending = {
    executionId: "retry",
    text: "Try again",
    attachments: [],
    files: [],
    mode: "queued" as const,
  }
  const source = view("auth", { messages: [refusal] })
  expect(recovery(transcriptFrom(source, 1, at, GATEWAY_NOW_MS))).toBe(true)
  source.pending = [{ ...pending, executionId: refusal.executionId }]
  expect(recovery(transcriptFrom(source, 2, at, GATEWAY_NOW_MS))).toBe(true)
  source.pending.push(pending)
  expect(recovery(transcriptFrom(source, 3, at, GATEWAY_NOW_MS))).toBe(false)
  source.messages.push(turn({ executionId: pending.executionId, status: "running" }))
  expect(recovery(transcriptFrom(source, 4, at, GATEWAY_NOW_MS))).toBe(false)
  source.pending = []
  source.messages[1] = turn({
    executionId: pending.executionId,
    status: "completed",
    parts: [textPart("Ready", 0)],
  })
  expect(recovery(transcriptFrom(source, 5, at, GATEWAY_NOW_MS))).toBe(false)
})

it("retains an unknown Codex runtime agent independently of catalogue fallback", () => {
  const source = view("auth")
  source.runtime = { ...source.runtime!, agent: "codex", model: "unknown-codex-model" }
  source.messages = [turn({ status: "failed", authenticationRequired: true })]
  expect(runningModel(source)).toBeUndefined()
  expect(transcriptFrom(source, 1, at, GATEWAY_NOW_MS).agent).toBe("codex")
})

function recovery(transcript: ReturnType<typeof transcriptFrom>) {
  return offersAuthenticationRecovery(
    transcript.authenticationRefusal,
    transcript.latestInputId,
    [],
  )
}

describe("a turn the gateway says ended", () => {
  it("keeps a failure with no reply on the input, status and diagnostic apart", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [turn({ status: "failed", error: "Provider connection lost" })],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages).toHaveLength(1)
    expect(transcript.messages[0]).toMatchObject({
      role: "user",
      terminal: { status: "failed", error: "Provider connection lost" },
    })
    expect(messageText(transcript.messages[0]!)).toBe("Chart the sales")
  })

  it("keeps a cancellation on the reply and leaves the partial text as it arrived", () => {
    const transcript = transcriptFrom(
      view("c", {
        messages: [
          turn({
            status: "cancelled",
            error: "Stopped",
            parts: [textPart("Halfway", 0)],
          }),
        ],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.messages[0]).toMatchObject({ role: "user" })
    expect(transcript.messages[0]).not.toHaveProperty("terminal")
    expect(transcript.messages[1]).toMatchObject({
      role: "agent",
      terminal: { status: "cancelled", error: "Stopped" },
      parts: [{ kind: "text", text: "Halfway" }],
    })
  })

  it("omits a blank diagnostic and leaves a finished or still-running turn unmarked", () => {
    const unresolved = transcriptFrom(
      view("c", { messages: [turn({ status: "unresolved", error: "   " })] }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(unresolved.messages[0]).toMatchObject({
      terminal: { status: "unresolved" },
    })
    expect(unresolved.messages[0]).not.toHaveProperty("terminal.error")
    for (const status of ["queued", "running", "completed", "injected"] as const) {
      const transcript = transcriptFrom(
        view("c", { messages: [turn({ status, error: "should not show" })] }),
        1,
        at,
        GATEWAY_NOW_MS,
      )
      expect(
        transcript.messages.every(
          (message) => message.role === "note" || message.terminal === undefined,
        ),
      ).toBe(true)
    }
  })

  it("shows an OpenCode failure the gateway already reported", () => {
    const transcript = transcriptFrom(
      view("c", {
        runtime: {
          model: "x",
          provider: "x",
          workspace: "/",
          agent: "opencode",
          modelName: "x",
          contextWindowTokens: 1,
          reasoning: false,
        },
        messages: [turn({ status: "failed", error: "Provider connection lost" })],
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.agent).toBe("opencode")
    expect(transcript.messages[0]).toMatchObject({
      terminal: { status: "failed", error: "Provider connection lost" },
    })
  })
})

describe("questions and a withheld interaction", () => {
  const field = {
    key: "where",
    prompt: "Pick a place",
    multiSelect: false,
    freeText: false,
    required: true,
    options: [{ value: "here", label: "Here", description: "This machine" }],
  }

  /** A transcript of `listed`, each question it lists read whole. */
  const asked = (listed: ConversationView) =>
    transcriptFrom(
      listed,
      1,
      at,
      GATEWAY_NOW_MS,
      [],
      listed.questions.map((entry) => ({
        state: "loaded" as const,
        entry,
        shown: shownQuestion(entry.question)!,
      })),
    )

  it("lists a question not read yet as loading, asking nothing, until it has been read", () => {
    const question = {
      executionId: "e1",
      questionId: "q1",
      message: "Where should this land?",
      questions: [field],
    }
    const listed = view("c", {
      messages: [turn({ status: "running" })],
      questions: [question],
    })
    const [entry] = listed.questions
    const waiting = {
      key: questionKey(question),
      executionId: "",
      questionId: "",
      message: "Where should this land?",
      questions: [],
    }
    expect(transcriptFrom(listed, 1, at, GATEWAY_NOW_MS).questions).toEqual([
      { ...waiting, loading: "reading" },
    ])
    expect(
      transcriptFrom(
        listed,
        1,
        at,
        GATEWAY_NOW_MS,
        [],
        [{ state: "failed", entry: entry!, error: null }],
      ).questions,
    ).toEqual([{ ...waiting, loading: "failed" }])
    expect(asked(listed).questions).toEqual([
      expect.objectContaining({ key: questionKey(question), questionId: "q1" }),
    ])
  })

  it("keeps a question that names an ask, and the gateway's notice in its own words", () => {
    const transcript = asked(
      view("c", {
        messages: [turn({ status: "running" })],
        questions: [
          {
            executionId: "e1",
            questionId: "q1",
            message: "Where should this land?",
            questions: [field],
          },
        ],
        interactionViewError: "A review was left out of this view.",
      }),
    )
    expect(transcript.questions).toEqual([
      {
        key: questionKey({ executionId: "e1", questionId: "q1" }),
        executionId: "e1",
        questionId: "q1",
        message: "Where should this land?",
        questions: [
          {
            key: "where",
            prompt: "Pick a place",
            multiSelect: false,
            freeText: false,
            required: true,
            options: [{ value: "here", label: "Here", description: "This machine" }],
          },
        ],
      },
    ])
    expect(transcript.interactionNotice).toBe("A review was left out of this view.")
  })

  it("skips a question, field, or option that names nothing, and a blank notice", () => {
    const transcript = asked(
      view("c", {
        messages: [turn({ status: "running" })],
        questions: [
          {
            executionId: "",
            questionId: "q0",
            message: "Missing an execution",
            questions: [field],
          },
          { executionId: "e1", questionId: "q1", message: "   ", questions: [field] },
          {
            executionId: "e1",
            questionId: "q2",
            message: "No usable field",
            questions: [
              { ...field, key: "" },
              { ...field, prompt: "  " },
              { ...field, options: [] },
            ],
          },
          {
            executionId: "e1",
            questionId: "q3",
            message: "Where should this land?",
            questions: [
              {
                ...field,
                options: [
                  { value: "", label: "Nowhere" },
                  { value: "here", label: "   " },
                  { value: "there", label: "There" },
                ],
              },
            ],
          },
        ],
        interactionViewError: "  ",
      }),
    )
    expect(transcript.questions).toEqual([
      {
        key: questionKey({ executionId: "e1", questionId: "q3" }),
        executionId: "e1",
        questionId: "q3",
        message: "Where should this land?",
        questions: [
          {
            key: "where",
            prompt: "Pick a place",
            multiSelect: false,
            freeText: false,
            required: true,
            options: [{ value: "there", label: "There" }],
          },
        ],
      },
    ])
    expect(transcript.interactionNotice).toBeUndefined()
  })
})

describe("a conversation view's lease", () => {
  it("is carried as where the agent runs, and is absent until one is recorded", () => {
    expect(transcriptFrom(view("c", {}), 1, at, GATEWAY_NOW_MS).lease).toBeUndefined()
    const transcript = transcriptFrom(
      view("c", {
        lease: {
          state: "ended",
          revision: 2,
          environment: "here",
          sandbox: "harness_default",
          cause: "closed",
          cleanup: "confirmed",
          droppedEvents: 0,
        },
      }),
      1,
      at,
      GATEWAY_NOW_MS,
    )
    expect(transcript.lease).toEqual({
      state: "ended",
      environment: "here",
      cause: "closed",
      refusal: undefined,
    })
  })
})

describe("user attachment references across conversation reads", () => {
  const image = { digest: "a".repeat(64), mimeType: "image/png" as const, size: 2048 }
  const file = { path: "/workspace/quarterly [report].csv" }
  for (const phase of ["committed", "pending", "restored"] as const) {
    for (const kind of ["image", "file", "mixed"] as const) {
      it(`keeps ${kind} metadata in ${phase} inputs without changing their owner`, () => {
        const attachments = kind === "file" ? [] : [image]
        const files = kind === "image" ? [] : [file]
        const wire = view("original", {
          messages:
            phase === "pending"
              ? []
              : [
                  turn({
                    userText: kind === "mixed" ? " Inspect `this`. " : "",
                    attachments,
                    files,
                  }),
                ],
          pending:
            phase === "pending"
              ? [
                  {
                    executionId: "e1",
                    mode: "queued",
                    text: kind === "mixed" ? " Inspect `this`. " : "",
                    attachments,
                    files,
                  },
                ]
              : [],
        })
        const read = phase === "restored" ? JSON.parse(JSON.stringify(wire)) : wire
        const transcript = transcriptFrom(read, 1, at, GATEWAY_NOW_MS)
        expect(transcript.sessionId).toBe("original")
        expect(transcript.messages[0]).toMatchObject({
          id: inputId("e1"),
          role: "user",
          parts: [
            { kind: "text", text: kind === "mixed" ? " Inspect `this`. " : "" },
            {
              kind: "attachments",
              content: [
                ...attachments.map((item) => ({ type: "image-reference", ...item })),
                ...files.map((item) => ({ type: "file-reference", ...item })),
              ],
            },
          ],
        })
        expect(wire.messages[0]?.attachments ?? wire.pending[0].attachments).toEqual(
          attachments,
        )
      })
    }
  }
})

it("omits an internal turn's empty input while retaining its assistant reply", () => {
  const transcript = transcriptFrom(
    view("c", {
      messages: [turn({ userText: "", parts: [textPart("Task finished.", 0)] })],
    }),
    1,
    at,
    100,
  )
  expect(transcript.messages.filter((message) => message.role === "user")).toEqual([])
  expect(
    transcript.messages.some(
      (message) =>
        message.role === "agent" &&
        message.parts.some(
          (part) => part.kind === "text" && part.text === "Task finished.",
        ),
    ),
  ).toBe(true)
})
