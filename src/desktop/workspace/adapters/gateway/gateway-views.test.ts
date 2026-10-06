import { offersAuthenticationRecovery } from "../../../../provider-authentication/model/recovery"
import type {
  ConversationMessage,
  ConversationPermission,
  ConversationTool,
} from "@nessa/client"
import { describe, expect, it } from "vitest"
import { appWidget } from "../../../widgets/app/model/app-ref"
import { composerModels } from "../../../model/composer-options"
import { messageText } from "../../model/transcript"
import { defaultModel } from "../../model/workspace-index"
import {
  approvalId,
  inputId,
  modelFor,
  replyId,
  reviewOf,
  runningModel,
  summaryFrom,
  transcriptFrom,
} from "./gateway-views"
import { row, view } from "./fake-gateway"

const at = () => 42

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
    )
    const reply = transcript.messages.find((message) => message.role === "agent")
    expect(reply?.parts).toEqual([
      { kind: "step", step: "run", label: "Draw a chart" },
      { kind: "widget", widget: appWidget("charts", "c", "e1", "t1") },
      { kind: "step", step: "run", label: "List rows" },
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
    )
    expect(
      transcript.messages.map((message) => [message.id, messageText(message)]),
    ).toEqual([
      ["e1", "Chart the sales"],
      ["e2", "And costs"],
    ])
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
    )
    expect(working.activity).toEqual({ label: "Reading files", since: 42 })
    const thinking = transcriptFrom(
      view("c", { messages: [turn({ status: "running" })] }),
      1,
      at,
    )
    expect(thinking.activity?.label).toBe("Thinking")
    const streaming = transcriptFrom(
      view("c", { messages: [turn({ status: "running", parts: [textPart("Hi", 0)] })] }),
      1,
      at,
    )
    expect(streaming.activity).toBeNull()
    expect(transcriptFrom(view("c", { messages: [turn()] }), 1, at).activity).toBeNull()
  })

  it("asks the first pending permission as the approval, under an id that names the review", () => {
    const transcript = transcriptFrom(
      view("c", {
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
      }),
      1,
      at,
    )
    expect(transcript.approval).toMatchObject({
      // The exact input approved, whatever the provider titles the review.
      command: 'bash {"command":"rm -rf build"}',
      reason: "Tool permission",
      origin: { kind: "agent" },
      // The review's own answers, and no always: the view offered none.
      options: [{ id: "a", label: "Allow", choice: "once" }],
      ask: "tool",
    })
    expect(reviewOf(transcript.approval!.id)).toEqual({
      executionId: "e/1",
      permissionId: "p%2",
    })
  })
})

describe("who asks for an approval (#436)", () => {
  const asked = (
    origin: ConversationPermission["origin"],
    ask: ConversationPermission["ask"] = "tool",
  ) =>
    transcriptFrom(
      view("c", {
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
      }),
      1,
      at,
    ).approval

  it("carries each offered answer in the view's order and words, and none it did not offer (#444)", () => {
    const approval = asked({ kind: "harness" })
    // The fixture lists only allow. A deny the view does not carry is not invented,
    // and neither is always.
    expect(approval?.options).toEqual([{ id: "allow", label: "Allow", choice: "once" }])
    const both = transcriptFrom(
      view("c", {
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
      }),
      1,
      at,
    ).approval
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
    const id = approvalId({
      executionId: "x/y",
      permissionId: "",
    } as Parameters<typeof approvalId>[0])
    expect(reviewOf(id)).toEqual({ executionId: "x/y", permissionId: "" })
    for (const foreign of ["", "a", "a/b/c", "a%/b", "%zz/b"])
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
        runtime: {
          model,
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
  })

  it("is said as the known one, else the composer's default", () => {
    const known = { provider: "openai", modelId: "gpt-5" }
    expect(modelFor(known)).toBe(known)
    expect(modelFor(undefined)).toEqual(defaultModel())
  })
})

describe("provider authentication recovery", () => {
  it("uses only the latest turn's typed refusal, never its diagnostic text", () => {
    const source = view("auth")
    source.messages = [turn({ status: "failed", authenticationRequired: true })]
    expect(recovery(transcriptFrom(source, 1, at))).toBe(true)
    source.messages.push(turn({ executionId: "later" }))
    expect(recovery(transcriptFrom(source, 2, at))).toBe(false)
    source.messages = [turn({ status: "failed", error: "OAuth session expired" })]
    expect(recovery(transcriptFrom(source, 3, at))).toBe(false)
  })
})

it("shows auth recovery without duplicating its diagnostic and preserves unrelated notices", () => {
  const source = view("auth", {
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
  const transcript = transcriptFrom(source, 1, at)
  expect(recovery(transcript)).toBe(true)
  expect(transcript.messages.flatMap((message) => message.parts)).toEqual([
    { kind: "text", text: "Chart the sales" },
    { kind: "text", text: "Nessa declined a tool review." },
  ])
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
  expect(recovery(transcriptFrom(source, 1, at))).toBe(true)
  source.pending = [{ ...pending, executionId: refusal.executionId }]
  expect(recovery(transcriptFrom(source, 2, at))).toBe(true)
  source.pending.push(pending)
  expect(recovery(transcriptFrom(source, 3, at))).toBe(false)
  source.messages.push(turn({ executionId: pending.executionId, status: "running" }))
  expect(recovery(transcriptFrom(source, 4, at))).toBe(false)
  source.pending = []
  source.messages[1] = turn({
    executionId: pending.executionId,
    status: "completed",
    parts: [textPart("Ready", 0)],
  })
  expect(recovery(transcriptFrom(source, 5, at))).toBe(false)
})

it("retains an unknown Codex runtime agent independently of catalogue fallback", () => {
  const source = view("auth")
  source.runtime = { ...source.runtime!, agent: "codex", model: "unknown-codex-model" }
  source.messages = [turn({ status: "failed", authenticationRequired: true })]
  expect(runningModel(source)).toBeUndefined()
  expect(transcriptFrom(source, 1, at).agent).toBe("codex")
})

function recovery(transcript: ReturnType<typeof transcriptFrom>) {
  return offersAuthenticationRecovery(
    transcript.authenticationRefusal,
    transcript.latestInputId,
    [],
  )
}
