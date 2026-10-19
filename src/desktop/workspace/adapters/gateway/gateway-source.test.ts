// @vitest-environment jsdom
/**
 * The gateway source against a fake client, one or more tests per row of the
 * tables on #248 and #702 (row ids in the test names): connection (C), the
 * index and conversations (R, D), writes (W), refusals (F), the connect
 * rules of #419 (S), MCP Apps (#384) and an app's review (#436, P). The D
 * rows are the desktop rows of `docs/design/record-subscriptions.md`.
 */
import {
  NessaConnectionClosedError,
  NessaCredentialUnavailableError,
  RetryableConnectError,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  questionTargetConversation,
  type ConversationPermission,
} from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import { conversationView } from "../../../../../packages/nessa-client/src/protocol/conversation-validate"
import { HostRefusalError } from "../../../../host/startup-refusals"
import { SessionHealthError } from "../../../../session/adapters/client/dev-session"
import { WorkspaceSourceError, type WorkspaceUpdate } from "../../application/ports"
import { composerModels } from "../../../model/composer-options"
import { atWork, messageText } from "../../model/transcript"
import { testStore } from "../../testing"
import { followWorkspace, loadWorkspace, sendMessage } from "../store/commands"
import { approvalId } from "./gateway-views"
import {
  deferred,
  gatewayConversationIds,
  fakeGateway,
  GATEWAY_NOW_MS,
  questionKey,
  reviewEntry,
  row,
  view,
  type FakeGateway,
  type Unkeyed,
} from "./fake-gateway"
import { gatewaySource, refusalOf, type GatewayClock } from "./gateway-source"

const { a: conversationA, b: conversationB, s: conversationS } = gatewayConversationIds
const timing = { callMs: 5_000, retryMs: 1_000 }
const model = { provider: "anthropic", modelId: "claude-opus-5" }

/** A clock and timers the test moves by hand. */
function manualClock() {
  let now = 1_000_000
  let timers: { at: number; run: () => void; cancelled: boolean }[] = []
  const clock: GatewayClock = {
    now: () => now,
    after(ms, run) {
      const timer = { at: now + ms, run, cancelled: false }
      timers.push(timer)
      return () => {
        timer.cancelled = true
      }
    },
  }
  const advance = async (ms: number) => {
    const until = now + ms
    for (;;) {
      await flush()
      const due = timers
        .filter((timer) => !timer.cancelled && timer.at <= until)
        .sort((a, b) => a.at - b.at)[0]
      if (!due) break
      now = due.at
      due.cancelled = true
      due.run()
    }
    now = until
    timers = timers.filter((timer) => !timer.cancelled)
    await flush()
  }
  return { clock, advance }
}

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve()
}

function started(
  gateway: FakeGateway = fakeGateway(),
  connect: () => Promise<FakeGateway["client"]> = () => Promise.resolve(gateway.client),
) {
  const { clock, advance } = manualClock()
  const source = gatewaySource({ connect, clock, timing })
  const updates: WorkspaceUpdate[] = []
  const follow = () => source.subscribe((update) => updates.push(update))
  return { gateway, source, updates, follow, advance }
}

const runtime = (modelId: string) => ({
  model: modelId,
  provider: "x",
  workspace: "/",
  agent: "x",
  modelName: "x",
  contextWindowTokens: 1,
  reasoning: false,
})

const rpcCode = (code: string) => new NessaRpcError(code, "message text nobody parses")

const kinds = (updates: readonly WorkspaceUpdate[]) =>
  updates.map((update) => update.kind)

const quiet = () => vi.spyOn(console, "warn").mockImplementation(() => {})

/** The subscriptions opened for one conversation, in order: each one's options. */
const subscribesOf = (gateway: FakeGateway, id: string) =>
  gateway.calls
    .filter((call) => call.method === "subscribe" && call.args[0] === id)
    .map((call) => call.args[1])

const transcriptsOf = (updates: readonly WorkspaceUpdate[], id: string) =>
  updates.flatMap((update) =>
    update.kind === "transcript" && update.transcript.sessionId === id
      ? [update.transcript]
      : [],
  )

const permission = (
  change: Partial<ConversationPermission> = {},
): Unkeyed<ConversationPermission> => ({
  executionId: "turn",
  permissionId: "p1",
  toolId: "t1",
  title: "Run pnpm test",
  options: [
    // Listed deny first, and labelled nothing like their effect: the effect decides.
    { id: "opt-b", label: "Nope", effect: "deny" },
    { id: "opt-a", label: "Sure", effect: "allow" },
  ],
  toolName: "bash",
  origin: { kind: "harness" },
  ask: "tool",
  argumentsJson: "{}",
  ...change,
})

const running = (id = "turn") => ({
  executionId: id,
  userText: "Run the tests",
  attachments: [],
  files: [],
  status: "running" as const,
  parts: [],
})

describe("the index and the list subscription", () => {
  it("the default gateway fixture conforms to the client view contract", () => {
    expect(conversationView(view(conversationA), conversationA)).toEqual(
      view(conversationA),
    )
  })

  it("C1, R1: connected, lists every conversation in one section and channel, each at its first revision", async () => {
    const { gateway, source } = started()
    gateway.rows.set(conversationA, row(conversationA, { running: true }))
    gateway.rows.set(conversationB, row(conversationB, { title: null, preview: null }))
    const index = await source.index()
    expect(index.sections).toHaveLength(1)
    expect(index.channels).toEqual([
      expect.objectContaining({ sectionId: index.sections[0].id }),
    ])
    expect(index.sessions).toEqual([
      expect.objectContaining({
        id: conversationA,
        channelId: index.channels[0].id,
        title: "Title of a",
        status: "running",
        startedAt: 1_000,
        updatedAt: 2_000,
        pinned: false,
        unread: false,
        revision: 1,
      }),
      expect.objectContaining({
        id: conversationB,
        title: "",
        preview: "",
        status: "idle",
        revision: 1,
      }),
    ])
    expect(gateway.calls.map((call) => call.method)).toEqual(["subscribeList"])
  })

  it("R1: the index again keeps every revision, from the list already followed", async () => {
    const { gateway, source } = started()
    gateway.rows.set(conversationA, row(conversationA))
    await source.index()
    expect((await source.index()).sessions[0].revision).toBe(1)
    expect(gateway.count("subscribeList")).toBe(1)
  })

  it("D1: a listener opens one list subscription, and nothing is asked on a timer", async () => {
    const { gateway, follow, advance } = started()
    gateway.rows.set(conversationA, row(conversationA))
    follow()
    await advance(timing.callMs * 3)
    expect(gateway.calls.map((call) => call.method)).toEqual(["subscribeList"])
  })

  it("S1: a list frame says a changed summary at its next revision, and an unchanged one not at all", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    follow()
    await source.index()
    updates.length = 0
    gateway.publishList()
    await flush()
    expect(updates).toEqual([])
    gateway.rows.set(
      conversationA,
      row(conversationA, { preview: "Something new", updatedAtMs: 3_000 }),
    )
    gateway.publishList()
    await flush()
    expect(updates).toEqual([
      {
        kind: "session",
        session: expect.objectContaining({
          id: conversationA,
          preview: "Something new",
          revision: 2,
        }),
      },
    ])
  })

  it("R2: an incomplete list frame keeps the sessions it leaves out; a complete one takes them out", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.rows.set(conversationB, row(conversationB))
    await source.index()
    gateway.rows.delete(conversationB)
    gateway.complete = false
    gateway.publishList()
    await flush()
    expect((await source.index()).sessions.map((session) => session.id)).toEqual([
      conversationA,
      conversationB,
    ])
    expect(kinds(updates)).not.toContain("session-removed")
    gateway.complete = true
    gateway.publishList()
    await flush()
    expect((await source.index()).sessions.map((session) => session.id)).toEqual([
      conversationA,
    ])
    expect(updates).toContainEqual({
      kind: "session-removed",
      sessionId: conversationB,
      revision: 2,
    })
  })

  it("S2, S3: a session archived elsewhere is taken out, and listed again outranks its removal", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    follow()
    await source.index()
    updates.length = 0
    gateway.rows.delete(conversationA)
    gateway.publishList()
    await flush()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.publishList()
    await flush()
    expect(updates).toEqual([
      { kind: "session-removed", sessionId: conversationA, revision: 2 },
      {
        kind: "session",
        session: expect.objectContaining({ id: conversationA, revision: 3 }),
      },
    ])
  })

  it("D6: an incomplete list is every stored summary once observe finishes", async () => {
    const { gateway, source } = started()
    for (const id of [conversationA, conversationB, "c", "d", "e"])
      gateway.rows.set(id, row(id, { updatedAtMs: id.charCodeAt(0) * 1_000 }))
    gateway.listLimit = 2
    gateway.observePageSize = 1
    const ids = (await source.index()).sessions.map((session) => session.id).sort()
    expect(ids).toEqual([conversationA, conversationB, "c", "d", "e"])
    expect(gateway.count("subscribeList")).toBe(1)
    expect(gateway.count("observe")).toBe(5)
  })

  it("D18: every incomplete frame walks, so a row it left out that went is taken out though the rows it names are the same", async () => {
    const { gateway, source, follow } = started()
    for (const id of [conversationA, conversationB, "c", "d"])
      gateway.rows.set(id, row(id, { updatedAtMs: id.charCodeAt(0) * 1_000 }))
    gateway.listLimit = 2
    gateway.observePageSize = 1
    follow()
    await source.index()
    expect(gateway.count("observe")).toBe(4)
    // The oldest goes elsewhere: the frame names the same two as before,
    // and the list is still incomplete.
    gateway.rows.delete(conversationA)
    gateway.publishList()
    await flush()
    expect(gateway.count("observe")).toBe(7)
    expect((await source.index()).sessions.map((session) => session.id).sort()).toEqual([
      conversationB,
      "c",
      "d",
    ])
  })

  it("D18: frames that come while a walk is on its way give way to the newest, which walks once", async () => {
    const { gateway, source, follow } = started()
    for (const id of [conversationA, conversationB, "c"])
      gateway.rows.set(id, row(id, { updatedAtMs: id.charCodeAt(0) * 1_000 }))
    gateway.listLimit = 2
    gateway.observePageSize = 10
    follow()
    await source.index()
    expect(gateway.count("observe")).toBe(1)
    const held = deferred<void>()
    gateway.once("observe", (normal) => held.promise.then(normal))
    gateway.publishList()
    await flush()
    expect(gateway.count("observe")).toBe(2)
    gateway.rows.set("d", row("d", { updatedAtMs: 1 }))
    gateway.publishList()
    gateway.publishList()
    gateway.publishList()
    held.resolve()
    await flush()
    expect(gateway.count("observe")).toBe(3)
    expect((await source.index()).sessions.map((session) => session.id).sort()).toEqual([
      conversationA,
      conversationB,
      "c",
      "d",
    ])
  })

  it("D18: a walk that fails leaves the list unapplied, a gap, and walks again on the retry clock", async () => {
    const { gateway, source, updates, follow, advance } = started()
    for (const id of [conversationA, conversationB, "c", "d"])
      gateway.rows.set(id, row(id, { updatedAtMs: id.charCodeAt(0) * 1_000 }))
    gateway.listLimit = 2
    gateway.observePageSize = 10
    gateway.once("observe", () => Promise.reject(rpcCode("unavailable")))
    follow()
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    expect(gateway.count("observe")).toBe(1)
    updates.length = 0
    await advance(timing.retryMs - 1)
    expect(gateway.count("observe")).toBe(1)
    await advance(1)
    // The same subscription; its frame walked again, and the gap resynced.
    expect(gateway.count("subscribeList")).toBe(1)
    expect(gateway.count("observe")).toBe(2)
    expect(updates).toContainEqual({ kind: "resync" })
    expect((await source.index()).sessions.map((session) => session.id).sort()).toEqual([
      conversationA,
      conversationB,
      "c",
      "d",
    ])
  })

  it("D18: an index asked after a walk failed walks again at once", async () => {
    const { gateway, source } = started()
    for (const id of [conversationA, conversationB, "c", "d"])
      gateway.rows.set(id, row(id, { updatedAtMs: id.charCodeAt(0) * 1_000 }))
    gateway.listLimit = 2
    gateway.observePageSize = 10
    gateway.once("observe", () => Promise.reject(rpcCode("unavailable")))
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    expect((await source.index()).sessions.map((session) => session.id).sort()).toEqual([
      conversationA,
      conversationB,
      "c",
      "d",
    ])
    expect(gateway.count("subscribeList")).toBe(1)
    expect(gateway.count("observe")).toBe(2)
  })

  it("a listener who leaves during an observe walk is asked no further page", async () => {
    const { gateway, source, follow } = started()
    gateway.rows.set(conversationA, row(conversationA, { updatedAtMs: 1 }))
    gateway.rows.set(conversationB, row(conversationB, { updatedAtMs: 2 }))
    gateway.rows.set("c", row("c", { updatedAtMs: 3 }))
    gateway.listLimit = 1
    gateway.observePageSize = 1
    const held = deferred<void>()
    gateway.once("observe", (normal) => held.promise.then(normal))
    const stop = follow()
    await flush()
    expect(gateway.count("observe")).toBe(1)
    stop()
    held.resolve()
    await flush()
    expect(gateway.count("observe")).toBe(1)
    expect((await source.index()).sessions.map((session) => session.id).sort()).toEqual([
      conversationA,
      conversationB,
      "c",
    ])
  })

  it("an observe page that does not finish keeps a summary it left out", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.rows.set(conversationA, row(conversationA, { updatedAtMs: 1 }))
    gateway.rows.set(conversationB, row(conversationB, { updatedAtMs: 2 }))
    gateway.rows.set("c", row("c", { updatedAtMs: 3 }))
    await source.index()
    expect(gateway.count("observe")).toBe(0)
    gateway.rows.delete(conversationB)
    gateway.complete = false
    gateway.listLimit = 1
    gateway.observePageSize = 1
    gateway.observeStops = true
    gateway.publishList()
    await flush()
    expect((await source.index()).sessions.map((session) => session.id).sort()).toEqual([
      conversationA,
      conversationB,
      "c",
    ])
    expect(kinds(updates)).not.toContain("session-removed")
    expect(gateway.count("observe")).toBe(1)
  })

  it("D9: a list subscription that ends is a gap: opened again on the retry clock, never at once, and its first frame resyncs once", async () => {
    const { gateway, source, updates, follow, advance } = started()
    follow()
    await source.index()
    updates.length = 0
    gateway.end("list", { reason: "source_closed" })
    await advance(timing.retryMs - 1)
    expect(gateway.count("subscribeList")).toBe(1)
    await advance(1)
    expect(gateway.count("subscribeList")).toBe(2)
    expect(updates).toEqual([{ kind: "resync" }])
    gateway.publishList()
    await flush()
    expect(updates).toEqual([{ kind: "resync" }])
  })

  it("D3 (list): a lagging list subscription is opened again at once, and is no gap", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    await source.index()
    gateway.end("list", { reason: "lagging" })
    await flush()
    expect(gateway.count("subscribeList")).toBe(2)
    expect(kinds(updates)).not.toContain("resync")
  })

  it("S9′: an index refused on a held client is a gap the next list frame resyncs", async () => {
    const { gateway, source, follow, advance, updates } = started()
    follow()
    await source.index()
    // The list opened again at once after it lagged is refused; so is the index.
    for (let i = 0; i < 2; i++)
      gateway.once("subscribeList", () =>
        Promise.reject(rpcCode("temporarily_unavailable")),
      )
    gateway.end("list", { reason: "lagging" })
    await flush()
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    updates.length = 0
    await advance(timing.retryMs)
    expect(kinds(updates)).toEqual(["resync"])
  })
})

describe("conversations", () => {
  it("says when the view left history out, and a later whole view replaces those messages", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    const turn = (executionId: string, userText: string, said: string) => ({
      executionId,
      userText,
      attachments: [],
      files: [],
      status: "completed" as const,
      parts: [{ offset: 0, kind: "text" as const, text: said, toolId: "", noticeId: "" }],
    })
    gateway.views.set(
      "a",
      view("a", {
        truncated: true,
        transcriptState: "stale",
        messages: [turn("e1", "recent", "still here")],
      }),
    )
    const first = await source.transcript("a")
    expect(first.history).toEqual({ omitted: true, unconfirmed: true })
    expect(first.messages.map(messageText)).toEqual(["recent", "still here"])
    gateway.views.set(
      "a",
      view("a", {
        truncated: false,
        transcriptState: "complete",
        messages: [turn("e2", "next", "the rest")],
      }),
    )
    gateway.publish("a")
    await flush()
    const replaced = transcriptsOf(updates, "a").at(-1)
    expect(replaced?.history).toBeUndefined()
    expect(replaced?.messages.map(messageText)).toEqual(["next", "the rest"])
    expect(new Set(replaced?.messages.map((message) => message.id)).size).toBe(
      replaced?.messages.length,
    )
  })

  it("R3: a transcript subscribes, sends no create, and reads at its first revision", async () => {
    const { gateway, source } = started()
    gateway.views.set(conversationA, view(conversationA))
    const transcript = await source.transcript(conversationA)
    expect(transcript).toMatchObject({ sessionId: conversationA, revision: 1 })
    expect(gateway.calls).toEqual([{ method: "subscribe", args: [conversationA, {}] }])
  })

  it("D20: a transcript of a conversation followed is the view held, with no second subscription", async () => {
    const { gateway, source } = started()
    gateway.views.set(conversationA, view(conversationA))
    await source.transcript(conversationA)
    expect(await source.transcript(conversationA)).toMatchObject({
      sessionId: conversationA,
      revision: 1,
    })
    expect(gateway.count("subscribe")).toBe(1)
  })

  it("R6, D2: a frame a person would see differently is the next transcript, whatever its revision; one that differs only in its revision is the same transcript", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.views.set(conversationA, view(conversationA, { revision: "x" }))
    expect((await source.transcript(conversationA)).revision).toBe(1)
    gateway.publish(conversationA)
    await flush()
    expect(transcriptsOf(updates, conversationA)).toHaveLength(1)
    // A fresh fold numbers the same content anew: nothing to say.
    gateway.views.set(conversationA, view(conversationA, { revision: "y" }))
    gateway.publish(conversationA)
    await flush()
    expect(transcriptsOf(updates, conversationA)).toHaveLength(1)
    // A live fact changes without the fold's revision: said.
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "y", title: "Renamed" }),
    )
    gateway.publish(conversationA)
    await flush()
    expect(transcriptsOf(updates, conversationA).map((said) => said.revision)).toEqual([
      1, 2,
    ])
    expect((await source.transcript(conversationA)).revision).toBe(2)
  })

  it("D2: a frame that differs only in the gateway's time it was stamped with is the same transcript", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.views.set(conversationA, view(conversationA))
    await source.transcript(conversationA)
    gateway.views.set(
      conversationA,
      view(conversationA, { serverNowMs: GATEWAY_NOW_MS + 5_000 }),
    )
    gateway.publish(conversationA)
    await flush()
    expect(transcriptsOf(updates, conversationA)).toHaveLength(1)
  })

  it("a review's deadline is put on the window's clock as its frame arrives, whatever the gateway's clock reads", async () => {
    // The window's clock is the manual one, far behind the gateway's.
    const { gateway, source } = started()
    gateway.views.set(
      conversationA,
      view(conversationA, {
        messages: [running()],
        permissions: [
          permission({
            deadline: {
              waitMs: 30_000,
              dueAtMs: GATEWAY_NOW_MS + 30_000,
              effect: "deny",
              reason: "No human answered in time.",
              stopsTurn: false,
            },
          }),
        ],
      }),
    )
    await source.transcript(conversationA)
    // Read whole from its live owner, then put on the window's clock.
    await flush()
    const transcript = await source.transcript(conversationA)
    expect(transcript.approvals[0]?.deadline?.dueAtMs).toBe(1_000_000 + 30_000)
  })

  it("a summary says a session waits on the person when its conversation holds a question", async () => {
    const { gateway, source, updates, follow } = started()
    const question = {
      executionId: "turn",
      questionId: "q1",
      message: "Where should this land?",
      questions: [
        {
          key: "where",
          prompt: "Pick a place",
          multiSelect: false,
          freeText: false,
          required: true,
          options: [{ value: "here", label: "Here" }],
        },
      ],
    }
    const otherQuestion = {
      ...question,
      questionId: "q2",
      message: "What priority should this have?",
      questions: [
        {
          ...question.questions[0],
          key: "priority",
          prompt: "Pick a priority",
          options: [{ value: "high", label: "High" }],
        },
      ],
    }
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set(
      "a",
      view("a", { messages: [running()], questions: [question, otherQuestion] }),
    )
    await source.index()
    follow()
    await source.transcript("a")
    await flush()
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({ id: "a", status: "needs-you", revision: 2 }),
    })
    expect(transcriptsOf(updates, "a").at(-1)?.questions).toEqual([
      expect.objectContaining({ questionId: "q1", message: "Where should this land?" }),
      expect.objectContaining({
        questionId: "q2",
        message: "What priority should this have?",
      }),
    ])
    await source.answerQuestion(
      "a",
      "turn",
      "q1",
      [{ key: "where", values: ["here"] }],
      "person",
      questionKey(question),
    )
    const answerCall = gateway.calls.find((call) => call.method === "answerQuestion")
    if (!answerCall) throw new Error("expected the gateway to answer the question")
    expect(questionTargetConversation(answerCall.args[0])).toBe("a")
    expect(answerCall.args[0]).toMatchObject({
      executionId: "turn",
      questionId: "q1",
      message: "Where should this land?",
      questions: [{ key: "where", options: [{ value: "here", label: "Here" }] }],
    })
    expect(answerCall?.args[1]).toEqual([
      { key: "where", values: ["here"], ownWords: null },
    ])
    gateway.publish("a")
    await flush()
    expect(transcriptsOf(updates, "a").at(-1)?.questions).toEqual([
      expect.objectContaining({ questionId: "q2" }),
    ])
  })

  it("declines a question with a null answer and refuses one the view does not list", async () => {
    const { gateway, source } = started()
    const asked = {
      executionId: "turn",
      questionId: "q1",
      message: "Where should this land?",
      questions: [
        {
          key: "where",
          prompt: "Pick a place",
          multiSelect: false,
          freeText: false,
          required: false,
          options: [{ value: "here", label: "Here" }],
        },
      ],
    }
    gateway.views.set("a", view("a", { messages: [running()], questions: [asked] }))
    await source.transcript("a")
    await flush()
    await source.answerQuestion("a", "turn", "q1", null, "agent", questionKey(asked))
    expect(gateway.calls.find((call) => call.method === "answerQuestion")?.args[1]).toBe(
      null,
    )
    gateway.publish("a")
    await flush()
    await expect(
      source.answerQuestion("a", "turn", "q1", null, "person", questionKey(asked)),
    ).rejects.toMatchObject({ reason: "not-waiting" })
    expect(gateway.count("answerQuestion")).toBe(1)
  })

  it("stops the execution the gateway's peek names, in a view that left a review out", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set(
      "a",
      view("a", {
        messages: [running()],
        truncated: true,
        interactionViewError:
          "A pending tool review exceeds the display limit; no choices were silently removed.",
      }),
    )
    gateway.active.set("a", "turn")
    await source.index()
    follow()
    const shown = await source.transcript("a")
    expect(shown.approvals).toEqual([])
    expect(atWork(shown)).toBe(true)
    await source.stop("a", "person")
    // The conversation alone: the peek, not the view, picks the execution.
    expect(gateway.calls.find((call) => call.method === "stop")?.args).toEqual(["a"])
    gateway.publish("a")
    await flush()
    expect(atWork(transcriptsOf(updates, "a").at(-1))).toBe(false)
  })

  it("says there is nothing to stop when a message still says running after its turn stopped", async () => {
    const { gateway, source } = started()
    // The projection keeps a stopped turn running until its result is
    // committed; the peek knows no turn is active.
    gateway.views.set("a", view("a", { messages: [running()] }))
    expect(atWork(await source.transcript("a"))).toBe(true)
    await expect(source.stop("a", "person")).rejects.toMatchObject({
      reason: "not-waiting",
    })
    expect(gateway.count("stop")).toBe(1)
    expect(gateway.views.get("a")?.messages[0]?.status).toBe("running")
  })

  it("withdraws a turn the person queued when none is active, so it never runs", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a", { messages: [{ ...running(), status: "queued" }] }))
    await source.index()
    follow()
    expect(atWork(await source.transcript("a"))).toBe(true)
    await source.stop("a", "person")
    const withdrawn = gateway.calls.filter((call) => call.method === "withdraw")
    expect(
      withdrawn.map((call) => (call.args[0] as { executionId: string }).executionId),
    ).toEqual(["turn"])
    gateway.publish("a")
    await flush()
    expect(atWork(transcriptsOf(updates, "a").at(-1))).toBe(false)
  })

  it("withdraws the queued turns before stopping the active one", async () => {
    const { gateway, source } = started()
    gateway.views.set(
      "a",
      view("a", {
        messages: [running("now"), { ...running("next"), status: "queued" }],
      }),
    )
    gateway.active.set("a", "now")
    await source.transcript("a")
    await source.stop("a", "person")
    expect(
      gateway.calls
        .filter((call) => call.method === "withdraw" || call.method === "stop")
        .map((call) => [
          call.method,
          call.method === "stop"
            ? call.args[0]
            : (call.args[0] as { executionId: string }).executionId,
        ]),
    ).toEqual([
      ["withdraw", "next"],
      ["stop", "a"],
    ])
    expect(gateway.views.get("a")?.messages.map((turn) => turn.status)).toEqual([
      "cancelled",
      "cancelled",
    ])
  })

  it("maps synchronous stale-choice validation without mapping later TypeErrors", async () => {
    const { gateway, source } = started()
    const question = {
      executionId: "turn",
      questionId: "q1",
      message: "Where should this land?",
      questions: [
        {
          key: "where",
          prompt: "Pick a place",
          multiSelect: false,
          freeText: false,
          required: true,
          options: [{ value: "here", label: "Here" }],
        },
      ],
    }
    gateway.views.set("a", view("a", { messages: [running()], questions: [question] }))
    await source.transcript("a")
    await flush()

    // Asked again under the same identities: a question of its own, listed
    // under its own key.
    const again = "c".repeat(32)
    gateway.views.set(
      "a",
      view("a", {
        messages: [running()],
        questions: [
          {
            ...question,
            question: again,
            questions: [
              {
                ...question.questions[0],
                options: [{ value: "there", label: "There" }],
              },
            ],
          },
        ],
      }),
    )
    gateway.publish("a")
    await flush()

    // The SDK's local target/choice validation throws before sending its RPC.
    const answerQuestion = vi
      .spyOn(gateway.client.conversation, "answerQuestion")
      .mockImplementation(() => {
        throw new TypeError("Select a field and choice offered by the captured question")
      })
    await expect(
      source.answerQuestion(
        "a",
        "turn",
        "q1",
        [{ key: "where", values: ["here"] }],
        "person",
        again,
      ),
    ).rejects.toMatchObject({ reason: "not-waiting" })

    expect(answerQuestion).toHaveBeenCalledWith(
      expect.objectContaining({
        executionId: "turn",
        questionId: "q1",
        questions: [
          expect.objectContaining({
            key: "where",
            options: [{ value: "there", label: "There" }],
          }),
        ],
      }),
      [{ key: "where", values: ["here"], ownWords: null }],
    )
    expect(gateway.count("answerQuestion")).toBe(0)

    answerQuestion.mockRestore()
    gateway.once("answerQuestion", () => Promise.reject(new TypeError("late failure")))
    await expect(
      source.answerQuestion(
        "a",
        "turn",
        "q1",
        [{ key: "where", values: ["there"] }],
        "person",
        again,
      ),
    ).rejects.toBeInstanceOf(TypeError)
    expect(gateway.count("answerQuestion")).toBe(1)
  })

  it("shows a withheld-review notice without saying the session waits on the person", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set("a", row("a"))
    gateway.views.set(
      "a",
      view("a", {
        messages: [{ ...running(), status: "completed" }],
        interactionViewError: "A review was left out of this view.",
      }),
    )
    await source.index()
    follow()
    const transcript = await source.transcript("a")
    expect(transcript.interactionNotice).toBe("A review was left out of this view.")
    expect(updates).not.toContainEqual({
      kind: "session",
      session: expect.objectContaining({ id: "a", status: "needs-you" }),
    })
  })

  it("a summary says a session waits on the person when its conversation asks", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA, { running: true }))
    gateway.views.set(
      conversationA,
      view(conversationA, { messages: [running()], permissions: [permission()] }),
    )
    await source.index()
    follow()
    await source.transcript(conversationA)
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({
        id: conversationA,
        status: "needs-you",
        revision: 2,
      }),
    })
  })

  it("a summary is said again when what its conversation runs on comes to be known", async () => {
    const { gateway, source, updates, follow } = started()
    const listed = composerModels[composerModels.length - 1]
    gateway.rows.set(conversationA, row(conversationA))
    await source.index()
    follow()
    gateway.views.set(
      conversationA,
      view(conversationA, { runtime: runtime(listed.modelId) }),
    )
    await source.transcript(conversationA)
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({
        revision: 2,
        model: { provider: listed.provider, modelId: listed.modelId },
      }),
    })
  })

  it("D3: a lagging subscription is opened again at once from the cursor last applied", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.views.set(conversationA, view(conversationA))
    await source.transcript(conversationA)
    gateway.publish(conversationA)
    gateway.publish(conversationA)
    await flush()
    gateway.end(conversationA, {
      reason: "lagging",
      lastDelivered: gateway.cursor(conversationA),
    })
    await flush()
    expect(subscribesOf(gateway, conversationA)).toEqual([
      {},
      { after: { incarnation: "history-1", position: "3" } },
    ])
    expect(kinds(updates)).not.toContain("resync")
  })

  it("D8: a frame behind the cursor applied in the same history is not applied; one from another history is", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.views.set(conversationA, view(conversationA, { revision: "1", title: "1" }))
    await source.transcript(conversationA)
    gateway.views.set(conversationA, view(conversationA, { revision: "2", title: "2" }))
    gateway.publish(conversationA)
    await flush()
    gateway.rewind(conversationA, 0)
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "behind", title: "behind" }),
    )
    gateway.publish(conversationA)
    await flush()
    expect(transcriptsOf(updates, conversationA).map((said) => said.revision)).toEqual([
      1, 2,
    ])
    gateway.incarnation = "history-2"
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "replaced", title: "replaced" }),
    )
    gateway.publish(conversationA)
    await flush()
    expect(transcriptsOf(updates, conversationA).map((said) => said.revision)).toEqual([
      1, 2, 3,
    ])
  })

  it("D19: a cursor ahead of the history the gateway holds opens once more without it", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.views.set(conversationA, view(conversationA))
    await source.transcript(conversationA)
    gateway.publish(conversationA)
    await flush()
    gateway.rewind(conversationA, 1)
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    gateway.setState({ status: "connected" })
    await flush()
    expect(subscribesOf(gateway, conversationA)).toEqual([
      {},
      { after: { incarnation: "history-1", position: "2" } },
      {},
    ])
    expect(gateway.live(conversationA)).toBe(1)
    // Started over: the view is said again, at the conversation's next count.
    expect(transcriptsOf(updates, conversationA).map((said) => said.revision)).toEqual([
      1, 2,
    ])
    expect(kinds(updates)).not.toContain("session-removed")
  })

  it("D4: a subscription the gateway ends as not found takes the session out, and is opened no more", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set(conversationA, row(conversationA, { running: true }))
    gateway.views.set(conversationA, view(conversationA))
    // An incomplete list, which proves nothing of what it leaves out.
    gateway.complete = false
    follow()
    await source.index()
    await source.transcript(conversationA)
    gateway.end(conversationA, { reason: "refused", code: "conversation_not_found" })
    await flush()
    expect(updates).toContainEqual({
      kind: "session-removed",
      sessionId: conversationA,
      revision: 2,
    })
    expect((await source.index()).sessions).toEqual([])
    await expect(source.transcript(conversationA)).rejects.toMatchObject({
      reason: "unknown-session",
    })
    await advance(timing.retryMs * 3)
    expect(gateway.count("subscribe")).toBe(1)
    expect(kinds(updates)).not.toContain("resync")
  })

  it("F1: a conversation the gateway does not hold is an unknown session, taken out", async () => {
    const { source, updates, follow } = started()
    follow()
    await expect(source.transcript("missing")).rejects.toMatchObject({
      reason: "unknown-session",
    })
    expect(kinds(updates)).toEqual(["session-removed"])
  })

  it("D10: a subscription that ends too large, or with a frame the client could not read, is let go: no retry, no gap; asked again it follows again", async () => {
    for (const reason of ["too_large", "invalid_frame"] as const) {
      const warn = quiet()
      const { gateway, source, updates, follow, advance } = started()
      gateway.views.set(conversationA, view(conversationA))
      follow()
      await source.transcript(conversationA)
      gateway.end(conversationA, { reason })
      await advance(timing.retryMs * 3)
      expect(gateway.count("subscribe")).toBe(1)
      expect(kinds(updates)).not.toContain("resync")
      expect(warn).toHaveBeenCalled()
      await expect(source.transcript(conversationA)).resolves.toMatchObject({
        sessionId: conversationA,
      })
      expect(gateway.count("subscribe")).toBe(2)
      warn.mockRestore()
    }
  })

  it("R11: a subscription refused for good is not followed, is no gap, and Try Again follows it again", async () => {
    const warn = quiet()
    const { gateway, source, updates, follow, advance } = started()
    gateway.views.set(conversationA, view(conversationA))
    follow()
    gateway.once("subscribe", () =>
      Promise.reject(rpcCode("conversation_state_unreadable")),
    )
    await expect(source.transcript(conversationA)).rejects.toMatchObject({
      reason: "not-supported",
    })
    await advance(timing.retryMs * 3)
    expect(gateway.count("subscribe")).toBe(1)
    expect(kinds(updates)).not.toContain("resync")
    await expect(source.transcript(conversationA)).resolves.toMatchObject({
      sessionId: conversationA,
    })
    expect(gateway.live(conversationA)).toBe(1)
    warn.mockRestore()
  })

  it("R3, D15: a subscription that could not open is opened again on the retry clock, and the next list frame resyncs", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set(conversationA, row(conversationA, { running: true }))
    gateway.views.set(conversationA, view(conversationA))
    follow()
    await source.index()
    gateway.once("subscribe", () => Promise.reject(rpcCode("temporarily_unavailable")))
    await expect(source.transcript(conversationA)).rejects.toMatchObject({
      reason: "unavailable",
    })
    await advance(timing.retryMs - 1)
    expect(gateway.count("subscribe")).toBe(1)
    await advance(1)
    expect(gateway.count("subscribe")).toBe(2)
    expect(transcriptsOf(updates, conversationA)).toEqual([
      expect.objectContaining({ sessionId: conversationA, revision: 1 }),
    ])
    gateway.publishList()
    await flush()
    expect(kinds(updates)).toContain("resync")
  })

  it("D16: a subscription that ends as the gateway stops it waits the retry clock, then opens from its cursor", async () => {
    const { gateway, source, follow, advance } = started()
    gateway.views.set(conversationA, view(conversationA))
    follow()
    await source.transcript(conversationA)
    gateway.end(conversationA, { reason: "source_closed" })
    await flush()
    expect(gateway.count("subscribe")).toBe(1)
    await advance(timing.retryMs)
    expect(subscribesOf(gateway, conversationA)).toEqual([
      {},
      { after: { incarnation: "history-1", position: "1" } },
    ])
  })

  it("D17: the retry clock runs only while someone listens, and stops when the last one leaves", async () => {
    const { gateway, source, follow, advance } = started()
    gateway.views.set(conversationA, view(conversationA))
    gateway.views.set(conversationB, view(conversationB))
    // No listener: nothing is opened again.
    gateway.once("subscribe", () => Promise.reject(rpcCode("temporarily_unavailable")))
    await expect(source.transcript(conversationA)).rejects.toMatchObject({
      reason: "unavailable",
    })
    await advance(timing.retryMs * 3)
    expect(gateway.count("subscribe")).toBe(1)
    // A listener who leaves before the clock runs takes it with them.
    const stop = follow()
    await flush()
    gateway.once("subscribe", () => Promise.reject(rpcCode("temporarily_unavailable")))
    await expect(source.transcript(conversationB)).rejects.toMatchObject({
      reason: "unavailable",
    })
    const opened = gateway.count("subscribe") + gateway.count("subscribeList")
    stop()
    await advance(timing.retryMs * 3)
    expect(gateway.count("subscribe") + gateway.count("subscribeList")).toBe(opened)
  })

  it("D7: past the published limit the conversation opened least recently is let go, says only its row, and is followed again when opened", async () => {
    const { gateway, source, updates, follow } = started()
    const ids = ["c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"]
    for (const id of ids) {
      gateway.rows.set(id, row(id, { running: true }))
      gateway.views.set(id, view(id))
    }
    gateway.views.set(
      "c0",
      view("c0", { messages: [running()], permissions: [permission()] }),
    )
    follow()
    await source.index()
    for (const id of ids.slice(0, 8)) await source.transcript(id)
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({ id: "c0", status: "needs-you" }),
    })
    updates.length = 0
    await source.transcript("c8")
    await flush()
    expect(gateway.live("c0")).toBe(0)
    expect(gateway.count("unsubscribe")).toBe(1)
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({ id: "c0", status: "running" }),
    })
    await source.transcript("c0")
    await flush()
    expect(subscribesOf(gateway, "c0")).toHaveLength(2)
    expect(gateway.live("c1")).toBe(0)
    expect(ids.filter((id) => gateway.live(id) === 1)).toHaveLength(8)
  })

  it("D23, D25: a conversation let go while its open is on its way is subscribed again only after that open is closed, and so is the one that took its place", async () => {
    const { gateway, source, follow } = started()
    const ids = ["c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"]
    for (const id of ids) {
      gateway.rows.set(id, row(id))
      gateway.views.set(id, view(id))
    }
    follow()
    await source.index()
    const held = deferred<void>()
    gateway.once("subscribe", (normal) => held.promise.then(normal))
    const first = source.transcript("c0").catch((error: unknown) => error)
    for (const id of ids.slice(1, 8)) await source.transcript(id)
    // Past the limit: c0, still opening, is let go. The gateway counts it
    // until it is answered and closed, so c8 waits for that (D25).
    const eighth = source.transcript("c8")
    expect(await first).toMatchObject({ reason: "unavailable" })
    await flush()
    expect(subscribesOf(gateway, "c8")).toHaveLength(0)
    // Opened again (c1, answered, is let go and closed at once).
    const again = source.transcript("c0")
    await flush()
    expect(subscribesOf(gateway, "c0")).toHaveLength(1)
    // The first open is answered and closed; its close is held.
    const closing = deferred<void>()
    gateway.once("unsubscribe", (normal) => closing.promise.then(normal))
    const before = gateway.count("unsubscribe")
    held.resolve()
    await flush()
    expect(gateway.count("unsubscribe")).toBe(before + 1)
    expect(subscribesOf(gateway, "c0")).toHaveLength(1)
    expect(subscribesOf(gateway, "c8")).toHaveLength(0)
    closing.resolve()
    await again
    await eighth
    await flush()
    expect(subscribesOf(gateway, "c0")).toHaveLength(2)
    expect(subscribesOf(gateway, "c8")).toHaveLength(1)
    expect(gateway.live("c0")).toBe(1)
    expect(gateway.live("c8")).toBe(1)
  })

  it("D24: the last listener leaving while the list and a conversation are opening, then one back before they are answered, opens both again once the old opens are closed", async () => {
    const { gateway, source, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    const listHeld = deferred<void>()
    const viewHeld = deferred<void>()
    gateway.once("subscribeList", (normal) => listHeld.promise.then(normal))
    gateway.once("subscribe", (normal) => viewHeld.promise.then(normal))
    const stop = follow()
    const shown = source.transcript(conversationA)
    await flush()
    expect(gateway.count("subscribeList")).toBe(1)
    expect(subscribesOf(gateway, conversationA)).toHaveLength(1)
    // Mounted, unmounted and mounted again, as StrictMode does.
    stop()
    follow()
    await flush()
    expect(gateway.count("subscribeList")).toBe(1)
    expect(subscribesOf(gateway, conversationA)).toHaveLength(1)
    listHeld.resolve()
    viewHeld.resolve()
    await flush()
    expect(gateway.count("unsubscribe")).toBe(2)
    expect(gateway.count("subscribeList")).toBe(2)
    expect(subscribesOf(gateway, conversationA)).toHaveLength(2)
    expect(gateway.live("list")).toBe(1)
    expect(gateway.live(conversationA)).toBe(1)
    expect((await shown).sessionId).toBe(conversationA)
    expect((await source.index()).sessions.map((session) => session.id)).toEqual([
      conversationA,
    ])
  })

  it("D13: a subscription answered after its session was taken out is closed unused, and its frame applies nothing", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    follow()
    await source.index()
    const held = deferred<void>()
    gateway.once("subscribe", (normal) => held.promise.then(normal))
    const shown = source.transcript(conversationA).catch((error: unknown) => error)
    await flush()
    await source.archive(conversationA, "person")
    expect(await shown).toMatchObject({ reason: "unknown-session" })
    held.resolve()
    await flush()
    expect(gateway.count("unsubscribe")).toBe(1)
    expect(gateway.live(conversationA)).toBe(0)
    expect(transcriptsOf(updates, conversationA)).toEqual([])
  })

  it("D14: a conversation opened, sent to by an app and restored at once is subscribed once", async () => {
    const { gateway, source, follow } = started()
    gateway.views.set(conversationA, view(conversationA))
    const shown = source.transcript(conversationA)
    const called = source.appCall(conversationA, () => Promise.resolve("done"))
    follow()
    await Promise.all([shown, called])
    await flush()
    expect(gateway.count("subscribe")).toBe(1)
  })
})

describe("every call settles on its own timer (C4)", () => {
  it("an index, a transcript, a send and an archive that get no answer settle unavailable", async () => {
    const { gateway, source, advance } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    gateway.views.set(conversationB, view(conversationB))
    await source.transcript(conversationA)
    for (const method of ["subscribeList", "subscribe", "send", "archive"] as const)
      gateway.once(method, () => new Promise(() => {}))
    const calls = [
      source.index(),
      source.transcript(conversationB),
      source.send({
        sessionId: conversationA,
        messageId: "m",
        text: "hi",
        model,
        initiator: "person",
      }),
      source.archive(conversationA, "person"),
    ].map((call) => call.catch((error: unknown) => error))
    await advance(timing.callMs)
    for (const settled of await Promise.all(calls))
      expect(settled).toMatchObject({ reason: "unavailable" })
  })

  it("a connection that never comes settles unavailable, and is tried again next time", async () => {
    const gateway = fakeGateway()
    let attempts = 0
    const { source, advance } = started(gateway, () => {
      attempts++
      return attempts === 1 ? new Promise(() => {}) : Promise.resolve(gateway.client)
    })
    const first = source.index().catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await first).toBeInstanceOf(WorkspaceSourceError)
    // Given up, not waited on: the next call makes a new attempt, which connects.
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
  })

  it("a client that connects after its attempt was given up is closed unused", async () => {
    const late = fakeGateway()
    const arriving = deferred<FakeGateway["client"]>()
    const { source, advance } = started(late, () => arriving.promise)
    const first = source.index().catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await first).toMatchObject({ reason: "unavailable" })
    arriving.resolve(late.client)
    await flush()
    expect(late.closed()).toBe(true)
    expect(late.count("subscribeList")).toBe(0)
  })

  it("a connection that fails is unavailable, and the next call connects again", async () => {
    const gateway = fakeGateway()
    const warn = quiet()
    let attempts = 0
    const { source } = started(gateway, () => {
      attempts++
      return attempts === 1
        ? Promise.reject(new Error("refused"))
        : Promise.resolve(gateway.client)
    })
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
    warn.mockRestore()
  })
})

describe("a connection that could not be made says why (#419)", () => {
  const failing = (error: unknown) => started(fakeGateway(), () => Promise.reject(error))
  const unauthorized = new NessaRpcError("unauthorized", "message text nobody parses")

  it("S2: no credential to present is unavailable: a configuration fault, not a sign-in", async () => {
    const warn = quiet()
    const { source } = failing(new NessaCredentialUnavailableError())
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    warn.mockRestore()
  })

  it("S3: a credential the gateway refused is signed out, at the handshake or the probe", async () => {
    const warn = quiet()
    for (const error of [
      unauthorized,
      new NessaConnectionClosedError(4001, ""),
      new SessionHealthError("probe", unauthorized),
    ])
      await expect(failing(error).source.index()).rejects.toMatchObject({
        reason: "signed-out",
      })
    warn.mockRestore()
  })

  it("S4: anything else — a host's sentence, a probe with no answer — is unavailable", async () => {
    const warn = quiet()
    for (const error of [
      new Error("The local server isn't ready yet"),
      new SessionHealthError("probe", new NessaConnectionClosedError(1006, "")),
      new SessionHealthError("probe", new RetryableConnectError("offline")),
    ])
      await expect(failing(error).source.index()).rejects.toMatchObject({
        reason: "unavailable",
      })
    warn.mockRestore()
  })

  it("a socket that never opens says the server is not answering", async () => {
    const warn = quiet()
    for (const error of [
      new RetryableConnectError("offline"),
      new NessaConnectionClosedError(1006, ""),
    ])
      await expect(failing(error).source.index()).rejects.toMatchObject({
        reason: "not-listening",
      })
    warn.mockRestore()
  })

  it("a typed host refusal names why the window has no server", async () => {
    const warn = quiet()
    await expect(
      failing(new HostRefusalError("not-provisioned")).source.index(),
    ).rejects.toMatchObject({ reason: "not-started" })
    await expect(
      failing(new HostRefusalError("not-ready")).source.index(),
    ).rejects.toMatchObject({
      reason: "not-ready",
    })
    await expect(
      failing(
        new HostRefusalError("wrong-stage", { bundle: "dev", requested: "prod" }),
      ).source.index(),
    ).rejects.toMatchObject({
      reason: "wrong-stage",
      stages: { bundle: "dev", requested: "prod" },
    })
    warn.mockRestore()
  })

  it("S5: a refusal that comes after the call budget changes nothing, and the next call asks again", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    const late = deferred<FakeGateway["client"]>()
    let attempts = 0
    const { source, advance } = started(gateway, () =>
      ++attempts === 1 ? late.promise : Promise.resolve(gateway.client),
    )
    const first = source.index().catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await first).toMatchObject({ reason: "unavailable" })
    late.reject(unauthorized)
    await flush()
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
    warn.mockRestore()
  })

  it("S6: every call sharing one attempt hears the same reason", async () => {
    const warn = quiet()
    const refusing = deferred<FakeGateway["client"]>()
    let attempts = 0
    const { source } = started(fakeGateway(), () => {
      attempts++
      return refusing.promise
    })
    const calls = [
      source.index(),
      source.transcript(conversationA),
      source.connected(),
    ].map((call) => call.catch((error: unknown) => error))
    refusing.reject(unauthorized)
    for (const settled of await Promise.all(calls))
      expect(settled).toMatchObject({ reason: "signed-out" })
    expect(attempts).toBe(1)
    warn.mockRestore()
  })

  it("S7: a message whose connection is refused is not sent, is refused, and says signed out", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    let attempts = 0
    const { source } = started(gateway, () =>
      ++attempts === 1 ? Promise.resolve(gateway.client) : Promise.reject(unauthorized),
    )
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    const store = testStore(source)
    await store.dispatch(loadWorkspace())
    await flush()
    // Gone for good; the next call connects again, and is refused.
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(4001, ""),
    })
    const outcome = await store.dispatch(
      sendMessage({ sessionId: conversationA, text: "Hello there", initiator: "person" }),
    )
    expect(outcome).toBe("refused")
    expect(gateway.count("send")).toBe(0)
    expect(store.getState().workspace.outbox[conversationA]?.[0]?.delivery).toMatchObject(
      {
        state: "failed",
        reason: "signed-out",
        model: store.getState().workspace.sessions[conversationA]?.model,
      },
    )
    warn.mockRestore()
  })

  it("S8: an index that cannot be read says signed out, and Try Again reads it again", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    let refuse = true
    const { source } = started(gateway, () =>
      refuse ? Promise.reject(unauthorized) : Promise.resolve(gateway.client),
    )
    gateway.rows.set(conversationA, row(conversationA))
    const store = testStore(source)
    await store.dispatch(loadWorkspace())
    expect(store.getState().workspace).toMatchObject({
      status: "failed",
      failure: "signed-out",
    })
    refuse = false
    await store.dispatch(loadWorkspace())
    expect(store.getState().workspace.status).toBe("ready")
    expect(Object.keys(store.getState().workspace.sessions)).toEqual([conversationA])
    warn.mockRestore()
  })

  it("S9: an index that failed is a gap: the retry clock's connect opens the workspace", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    let refuse = true
    const { source, advance } = started(gateway, () =>
      refuse
        ? Promise.reject(new NessaConnectionClosedError(1006, ""))
        : Promise.resolve(gateway.client),
    )
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    expect(store.getState().workspace).toMatchObject({
      status: "failed",
      failure: "not-listening",
    })
    refuse = false
    await advance(timing.retryMs - 1)
    expect(store.getState().workspace.status).toBe("failed")
    await advance(1)
    await flush()
    expect(store.getState().workspace.status).toBe("ready")
    expect(Object.keys(store.getState().workspace.sessions)).toEqual([conversationA])
    warn.mockRestore()
  })

  /** A source whose first connect succeeds, and every later one is refused as signed out. */
  function signedOutAfterFirst() {
    const gateway = fakeGateway()
    let attempts = 0
    const started_ = started(gateway, () =>
      ++attempts === 1 ? Promise.resolve(gateway.client) : Promise.reject(unauthorized),
    )
    return { ...started_, attempts: () => attempts }
  }

  it("S7: an answer whose connection is refused is not sent, and says signed out", async () => {
    const warn = quiet()
    const { gateway, source } = signedOutAfterFirst()
    gateway.views.set(
      conversationA,
      view(conversationA, { messages: [running()], permissions: [permission()] }),
    )
    await source.transcript(conversationA)
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(4001, ""),
    })
    await expect(
      source.approve(
        conversationA,
        approvalId(reviewEntry(permission())),
        "once",
        "person",
        "opt-a",
      ),
    ).rejects.toMatchObject({ reason: "signed-out" })
    expect(gateway.count("answer")).toBe(0)
    warn.mockRestore()
  })

  it("S7: an archive whose connection is refused is not sent, and says signed out", async () => {
    const warn = quiet()
    const { gateway, source } = signedOutAfterFirst()
    gateway.rows.set(conversationA, row(conversationA))
    await source.index()
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(4001, ""),
    })
    await expect(source.archive(conversationA, "person")).rejects.toMatchObject({
      reason: "signed-out",
    })
    expect(gateway.count("archive")).toBe(0)
    warn.mockRestore()
  })

  /** Connect attempts, counted, refused while `refuse` says so. */
  function counted(gateway = fakeGateway()) {
    const state = { attempts: 0, refuse: true }
    const started_ = started(gateway, () => {
      state.attempts++
      return state.refuse ? Promise.reject(unauthorized) : Promise.resolve(gateway.client)
    })
    return { ...started_, state }
  }

  it("S10: after a failed connect the subscriptions connect again only once the retry wait has passed", async () => {
    const warn = quiet()
    const { source, follow, advance, state } = counted()
    follow()
    await source.index().catch(() => undefined)
    expect(state.attempts).toBe(1)
    await advance(timing.retryMs - 1)
    expect(state.attempts).toBe(1)
    await advance(1)
    expect(state.attempts).toBe(2)
    await advance(timing.retryMs)
    expect(state.attempts).toBe(3)
    warn.mockRestore()
  })

  it("S12: a person's call connects at once while the subscriptions wait, and its failure starts the wait again", async () => {
    const warn = quiet()
    const { source, follow, advance, state } = counted()
    follow()
    await source.index().catch(() => undefined)
    await advance(timing.retryMs / 2)
    await expect(source.index()).rejects.toMatchObject({ reason: "signed-out" })
    expect(state.attempts).toBe(2)
    // The clock's turn comes inside the person's wait: it connects nothing.
    await advance(timing.retryMs / 2)
    expect(state.attempts).toBe(2)
    await advance(timing.retryMs)
    expect(state.attempts).toBe(3)
    warn.mockRestore()
  })

  it("S12: a connect that succeeds ends the wait", async () => {
    const warn = quiet()
    const { gateway, source, follow, advance, state } = counted()
    follow()
    await source.index().catch(() => undefined)
    state.refuse = false
    await source.index()
    state.refuse = true
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(1006, ""),
    })
    // No wait is left over: the clock's next turn connects.
    await advance(timing.retryMs)
    expect(state.attempts).toBe(3)
    warn.mockRestore()
  })

  it("S14: an MCP App's call while waiting is refused without connecting, and the wait is unchanged", async () => {
    const warn = quiet()
    const { source, follow, advance, state } = counted()
    follow()
    await source.index().catch(() => undefined)
    await advance(timing.retryMs / 2)
    for (let call = 0; call < 3; call++)
      await expect(source.connected()).rejects.toMatchObject({ reason: "unavailable" })
    expect(state.attempts).toBe(1)
    await advance(timing.retryMs / 2)
    expect(state.attempts).toBe(2)
    warn.mockRestore()
  })

  it("S17: a connect that outlasts the call budget starts the wait", async () => {
    const state = { attempts: 0 }
    const { source, follow, advance } = started(fakeGateway(), () => {
      state.attempts++
      return new Promise<FakeGateway["client"]>(() => {})
    })
    follow()
    const first = source.index().catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await first).toMatchObject({ reason: "unavailable" })
    // The index joined the subscriptions' one attempt; then the wait.
    expect(state.attempts).toBe(1)
    await advance(timing.retryMs - 1)
    expect(state.attempts).toBe(1)
    await advance(1)
    expect(state.attempts).toBe(2)
  })

  it("S18: while the source waits, apps and the retry clock join a person's connect on its way", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    let attempts = 0
    const pending = deferred<FakeGateway["client"]>()
    const { source, follow, advance } = started(gateway, () => {
      attempts++
      return attempts === 1 ? Promise.reject(unauthorized) : pending.promise
    })
    follow()
    await source.index().catch(() => undefined)
    const person = source.index()
    await flush()
    expect(attempts).toBe(2)
    const app = source.connected()
    await advance(timing.retryMs * 2)
    pending.resolve(gateway.client)
    await expect(app).resolves.toBe(gateway.client)
    await expect(person).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
    warn.mockRestore()
  })

  /**
   * A client whose handshake finished and whose connection is already closed.
   * `onConnectionStateChange` does not replay that close: the fake, like the
   * real client, only reports a later transition.
   */
  function closedClient(error: Error) {
    const gateway = fakeGateway()
    gateway.setState({ status: "closed", error })
    return gateway
  }

  it("S19: a client closed before adoption is not kept, and the next call connects", async () => {
    const warn = quiet()
    const dead = closedClient(new NessaConnectionClosedError(1006, ""))
    const live = fakeGateway()
    let attempts = 0
    const { source } = started(live, () => {
      attempts++
      return attempts === 1 ? Promise.resolve(dead.client) : Promise.resolve(live.client)
    })
    await expect(source.index()).rejects.toMatchObject({ reason: "not-listening" })
    expect(dead.closed()).toBe(true)
    expect(dead.count("subscribeList")).toBe(0)
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
    expect(live.count("subscribeList")).toBe(1)
    warn.mockRestore()
  })

  it("S19: a credential revoked before adoption is signed out, and the background wait still holds", async () => {
    const warn = quiet()
    const dead = closedClient(new NessaConnectionClosedError(4002, ""))
    const live = fakeGateway()
    let attempts = 0
    const { source, follow, advance } = started(live, () => {
      attempts++
      return attempts === 1 ? Promise.resolve(dead.client) : Promise.resolve(live.client)
    })
    follow()
    await expect(source.index()).rejects.toMatchObject({ reason: "signed-out" })
    expect(dead.closed()).toBe(true)
    expect(dead.count("subscribeList")).toBe(0)
    expect(attempts).toBe(1)
    // The failed attempt starts the same wait as any refused connect.
    await advance(timing.retryMs - 1)
    expect(attempts).toBe(1)
    await expect(source.connected()).rejects.toMatchObject({ reason: "unavailable" })
    expect(attempts).toBe(1)
    // A person's call does not wait out that budget.
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
    expect(live.count("subscribeList")).toBe(1)
    warn.mockRestore()
  })

  it("S19: a client that closes while adoption subscribes is refused as signed out", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    const revoked = new NessaConnectionClosedError(4002, "")
    const inner = gateway.client
    const client = {
      conversation: inner.conversation,
      subscriptions: inner.subscriptions,
      get connectionState() {
        return inner.connectionState
      },
      onConnectionStateChange(
        handler: (state: FakeGateway["client"]["connectionState"]) => void,
      ) {
        const off = inner.onConnectionStateChange(handler)
        // The close is reported before `current` can see it.
        gateway.setState({ status: "closed", error: revoked })
        return off
      },
      close() {
        inner.close()
      },
    }
    const live = fakeGateway()
    let attempts = 0
    const { source } = started(live, () => {
      attempts++
      return attempts === 1
        ? Promise.resolve(client as FakeGateway["client"])
        : Promise.resolve(live.client)
    })
    await expect(source.index()).rejects.toMatchObject({ reason: "signed-out" })
    expect(gateway.closed()).toBe(true)
    expect(gateway.count("subscribeList")).toBe(0)
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
    warn.mockRestore()
  })

  const waitingReview = (permissionId: string) => ({
    conversationId: "conversation-1",
    executionId: "execution-1",
    permissionId,
    tool: "Bash",
  })

  it("a connection that is not up clears waiting-review notices", async () => {
    const waiting: (readonly { conversationId: string }[])[] = []
    const gateway = fakeGateway()
    const { clock } = manualClock()
    const source = gatewaySource({
      connect: () => Promise.resolve(gateway.client),
      clock,
      timing,
      onReviewWaiting: (reviews) => waiting.push(reviews),
    })
    await source.index()
    expect(waiting).toEqual([])
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    expect(waiting).toEqual([[]])
    gateway.setState({
      status: "connected",
    })
    expect(waiting).toEqual([[]])
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(1006, ""),
    })
    expect(waiting).toEqual([[], []])
  })

  it("a reconnect restores waiting reviews the snapshot missed", async () => {
    const waiting: (readonly { permissionId: string }[])[] = []
    const gateway = fakeGateway()
    const { clock } = manualClock()
    const source = gatewaySource({
      connect: () => Promise.resolve(gateway.client),
      clock,
      timing,
      onReviewWaiting: (reviews) => waiting.push(reviews),
    })
    await source.index()
    const pending = waitingReview("permission-1")
    gateway.publishEvent("permission.reviewWaiting", { reviews: [pending] })
    expect(waiting).toEqual([[pending]])
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    expect(waiting).toEqual([[pending], []])
    // No snapshot arrives. The one dispatched before the listener was
    // attached is already gone, so the last set is put back.
    gateway.setState({ status: "connected" })
    expect(waiting).toEqual([[pending], [], [pending]])
  })

  it("a snapshot applied after the clear replaces the retained reviews", async () => {
    const waiting: (readonly { permissionId: string }[])[] = []
    const gateway = fakeGateway()
    const { clock } = manualClock()
    const source = gatewaySource({
      connect: () => Promise.resolve(gateway.client),
      clock,
      timing,
      onReviewWaiting: (reviews) => waiting.push(reviews),
    })
    await source.index()
    const pending = waitingReview("permission-1")
    const newer = waitingReview("permission-2")
    gateway.publishEvent("permission.reviewWaiting", { reviews: [pending] })
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    gateway.publishEvent("permission.reviewWaiting", { reviews: [newer] })
    gateway.setState({ status: "connected" })
    expect(waiting).toEqual([[pending], [], [newer]])
  })

  it("a later client restores waiting reviews the snapshot missed", async () => {
    const waiting: (readonly { permissionId: string }[])[] = []
    const first = fakeGateway()
    const second = fakeGateway()
    const clients = [first.client, second.client]
    let attempts = 0
    const { clock } = manualClock()
    const source = gatewaySource({
      connect: () => {
        const client = clients[attempts]
        attempts += 1
        return Promise.resolve(client!)
      },
      clock,
      timing,
      onReviewWaiting: (reviews) => waiting.push(reviews),
    })
    await source.index()
    const pending = waitingReview("permission-1")
    first.publishEvent("permission.reviewWaiting", { reviews: [pending] })
    first.setState({
      status: "closed",
      error: new NessaConnectionClosedError(1006, ""),
    })
    await source.index()
    expect(waiting).toEqual([[pending], [], [pending]])
    expect(attempts).toBe(2)
  })

  it("a later client restores waiting reviews unless its snapshot arrives with the listener", async () => {
    const waiting: (readonly { permissionId: string }[])[] = []
    const first = fakeGateway()
    const second = fakeGateway()
    const newer = waitingReview("permission-2")
    const inner = second.client
    const replaying = {
      conversation: inner.conversation,
      subscriptions: inner.subscriptions,
      get connectionState() {
        return inner.connectionState
      },
      onConnectionStateChange: inner.onConnectionStateChange.bind(inner),
      onEvent(event: string, handler: (payload: unknown) => void) {
        const listen = inner.onEvent
        if (!listen) return () => {}
        const off = listen(event, handler)
        if (event === "permission.reviewWaiting") handler({ reviews: [newer] })
        return off
      },
      close: inner.close.bind(inner),
    }
    const clients = [first.client, replaying]
    let attempts = 0
    const { clock } = manualClock()
    const source = gatewaySource({
      connect: () => {
        const client = clients[attempts]
        attempts += 1
        return Promise.resolve(client as FakeGateway["client"])
      },
      clock,
      timing,
      onReviewWaiting: (reviews) => waiting.push(reviews),
    })
    await source.index()
    const pending = waitingReview("permission-1")
    first.publishEvent("permission.reviewWaiting", { reviews: [pending] })
    first.setState({
      status: "closed",
      error: new NessaConnectionClosedError(1006, ""),
    })
    expect(waiting).toEqual([[pending], []])
    await source.index()
    expect(waiting).toEqual([[pending], [], [newer]])
    expect(attempts).toBe(2)
  })

  it("S19: a connect that is already closed after a live one is not kept", async () => {
    const warn = quiet()
    const first = fakeGateway()
    const dead = closedClient(new NessaConnectionClosedError(4002, ""))
    const live = fakeGateway()
    const clients = [first.client, dead.client, live.client]
    let attempts = 0
    const { source } = started(first, () => {
      const client = clients[attempts]
      attempts++
      return Promise.resolve(client!)
    })
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(first.count("subscribeList")).toBe(1)
    first.setState({
      status: "closed",
      error: new NessaConnectionClosedError(1006, ""),
    })
    await expect(source.index()).rejects.toMatchObject({ reason: "signed-out" })
    expect(dead.closed()).toBe(true)
    expect(dead.count("subscribeList")).toBe(0)
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(3)
    expect(live.count("subscribeList")).toBe(1)
    warn.mockRestore()
  })

  it("a client that is reconnecting when adopted is kept until that connection closes", async () => {
    const recovering = fakeGateway()
    recovering.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    const live = fakeGateway()
    let attempts = 0
    const { source } = started(live, () => {
      attempts++
      return attempts === 1
        ? Promise.resolve(recovering.client)
        : Promise.resolve(live.client)
    })
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(1)
    expect(recovering.closed()).toBe(false)
    expect(recovering.count("subscribeList")).toBe(1)
    recovering.setState({
      status: "closed",
      error: new NessaConnectionClosedError(1006, ""),
    })
    await expect(source.index()).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
    expect(live.count("subscribeList")).toBe(1)
  })

  for (const [path, call] of [
    [
      "opening a session",
      (source: ReturnType<typeof started>["source"]) => source.transcript(conversationA),
    ],
    [
      "a send",
      (source: ReturnType<typeof started>["source"]) =>
        source.send({
          sessionId: conversationA,
          messageId: "m",
          text: "hi",
          model,
          initiator: "person",
        }),
    ],
    [
      "an archive",
      (source: ReturnType<typeof started>["source"]) =>
        source.archive(conversationA, "person"),
    ],
  ] as const)
    it(`S12: ${path} connects at once while the source waits`, async () => {
      const warn = quiet()
      const gateway = fakeGateway()
      gateway.rows.set(conversationA, row(conversationA))
      gateway.views.set(
        conversationA,
        view(conversationA, { messages: [running()], permissions: [permission()] }),
      )
      const { source, state } = counted(gateway)
      await source.index().catch(() => undefined)
      state.refuse = false
      await call(source).catch(() => undefined)
      expect(state.attempts).toBe(2)
      warn.mockRestore()
    })

  it("S12: an answer connects at once while the source waits", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    gateway.rows.set(conversationA, row(conversationA))
    const asked = permission()
    gateway.views.set(
      conversationA,
      view(conversationA, { messages: [running()], permissions: [asked] }),
    )
    const { source, state } = counted(gateway)
    // Read whole first: only a review the window was shown is answered.
    state.refuse = false
    await source.transcript(conversationA)
    await flush()
    expect(state.attempts).toBe(1)
    // Then the connection goes, and connecting again is refused: it waits.
    state.refuse = true
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(1006, ""),
    })
    await source.index().catch(() => undefined)
    expect(state.attempts).toBe(2)
    // The gateway is back: the person's answer connects at once and is sent.
    state.refuse = false
    gateway.setState({ status: "connected" })
    await flush()
    expect(state.attempts).toBe(2)
    await source.approve(
      conversationA,
      approvalId(reviewEntry(asked)),
      "once",
      "person",
      "opt-a",
    )
    expect(state.attempts).toBe(3)
    expect(gateway.count("answer")).toBe(1)
    warn.mockRestore()
  })

  it("S15: a message's subscription uses the client held: after a close for good it connects nothing", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    const { source, state } = counted(gateway)
    state.refuse = false
    gateway.views.set(conversationA, view(conversationA))
    gateway.once("send", async (normal) => {
      const sent = await normal()
      state.refuse = true
      gateway.setState({
        status: "closed",
        error: new NessaConnectionClosedError(4001, ""),
      })
      return sent
    })
    await source.send({
      sessionId: conversationA,
      messageId: "m",
      text: "hi",
      model,
      initiator: "person",
    })
    await flush()
    expect(state.attempts).toBe(1)
    expect(gateway.count("subscribe")).toBe(0)
    warn.mockRestore()
  })

  it("S13: a connect that succeeds after dispose is closed unused, and its callers hear unavailable at once", async () => {
    const late = fakeGateway()
    const arriving = deferred<FakeGateway["client"]>()
    const { source } = started(late, () => arriving.promise)
    const index = source.index().catch((error: unknown) => error)
    await flush()
    source.dispose()
    arriving.resolve(late.client)
    await flush()
    expect(await index).toMatchObject({ reason: "unavailable" })
    expect(late.closed()).toBe(true)
    expect(late.count("subscribeList")).toBe(0)
  })

  it("S13: a connect refused after dispose is unavailable, and nothing is said of it", async () => {
    const warn = quiet()
    const refusing = deferred<FakeGateway["client"]>()
    const { source } = started(fakeGateway(), () => refusing.promise)
    const index = source.index().catch((error: unknown) => error)
    await flush()
    source.dispose()
    refusing.reject(unauthorized)
    expect(await index).toMatchObject({ reason: "unavailable" })
    expect(warn).not.toHaveBeenCalled()
    warn.mockRestore()
  })
})

describe("the connection", () => {
  it("C2, D5: a lost connection ends every subscription and nothing more; back, it resyncs once and opens each again from its cursor", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    follow()
    await source.index()
    await source.transcript(conversationA)
    gateway.publish(conversationA)
    await flush()
    updates.length = 0
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    await flush()
    expect(updates).toEqual([])
    expect(gateway.count("subscribe") + gateway.count("subscribeList")).toBe(2)
    gateway.setState({ status: "connected" })
    expect(updates).toEqual([{ kind: "resync" }])
    await flush()
    expect(gateway.count("subscribeList")).toBe(2)
    expect(subscribesOf(gateway, conversationA)).toEqual([
      {},
      { after: { incarnation: "history-1", position: "2" } },
    ])
    expect(kinds(updates)).toEqual(["resync"])
  })

  it("C3: a client closed for good is replaced on the next call, and that says resync", async () => {
    const gateway = fakeGateway()
    const next = fakeGateway()
    let attempts = 0
    const { source, updates, follow } = started(gateway, () =>
      Promise.resolve(++attempts === 1 ? gateway.client : next.client),
    )
    follow()
    await source.index()
    gateway.setState({ status: "closed", error: new Error("gone") })
    // The close drops the client and does not resync. The next client does.
    expect(updates).toEqual([])
    await source.index()
    expect(attempts).toBe(2)
    expect(updates).toEqual([{ kind: "resync" }])
    expect(next.count("subscribeList")).toBe(1)
  })

  it("C3: after a close for good the retry clock connects the next client, which resyncs and follows again", async () => {
    const gateway = fakeGateway()
    const next = fakeGateway()
    let attempts = 0
    const { source, updates, follow, advance } = started(gateway, () =>
      Promise.resolve(++attempts === 1 ? gateway.client : next.client),
    )
    gateway.views.set(conversationA, view(conversationA))
    next.views.set(conversationA, view(conversationA))
    follow()
    await source.index()
    await source.transcript(conversationA)
    updates.length = 0
    gateway.setState({ status: "closed", error: new Error("gone") })
    await advance(timing.retryMs)
    expect(attempts).toBe(2)
    expect(kinds(updates)).toEqual(["resync"])
    expect(next.count("subscribeList")).toBe(1)
    expect(subscribesOf(next, conversationA)).toEqual([
      { after: { incarnation: "history-1", position: "1" } },
    ])
  })

  it("C3, D22: dispose closes every subscription and the client, refuses later calls, and nothing applies after", async () => {
    const { gateway, source, updates, follow, advance } = started()
    follow()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    await source.index()
    await source.transcript(conversationA)
    source.dispose()
    await flush()
    expect(gateway.closed()).toBe(true)
    expect(gateway.count("unsubscribe")).toBe(2)
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    const said = updates.length
    gateway.views.set(conversationA, view(conversationA, { revision: "later" }))
    gateway.publish(conversationA)
    gateway.publishList()
    gateway.setState({ status: "connected" })
    await advance(timing.retryMs * 3)
    expect(updates).toHaveLength(said)
    expect(gateway.count("subscribeList")).toBe(1)
  })

  it("C3: a call in flight when the source is disposed settles unavailable", async () => {
    const { gateway, source } = started()
    const held = deferred<void>()
    gateway.once("subscribeList", (normal) => held.promise.then(normal))
    const index = source.index()
    await flush()
    source.dispose()
    held.resolve()
    await expect(index).rejects.toMatchObject({ reason: "unavailable" })
  })
})

describe("writes", () => {
  it("W1: a first message opens its conversation on the chosen agent and model, sends it under its id, then follows it", async () => {
    const { gateway, source } = started()
    await source.send({
      sessionId: conversationS,
      messageId: "m",
      text: "Hello",
      model: { provider: "openai", modelId: "gpt-5" },
      initiator: "person",
      start: { channelId: "gateway-conversations", title: "Hello" },
    })
    expect(gateway.calls).toEqual([
      {
        method: "create",
        args: [{ conversationId: conversationS, agent: "codex", model: "gpt-5" }],
      },
      {
        method: "send",
        args: [
          conversationS,
          "Hello",
          [],
          [],
          { executionId: "m", requestId: expect.any(String) },
        ],
      },
      { method: "subscribe", args: [conversationS, {}] },
    ])
  })

  it("W2: a message sent again goes under its turn's id, so the gateway takes it once, as a command of its own", async () => {
    const { gateway, source } = started()
    gateway.views.set(conversationS, view(conversationS))
    const message = {
      sessionId: conversationS,
      messageId: "m",
      text: "Hello",
      model,
      initiator: "person" as const,
    }
    gateway.once("send", () => Promise.reject(new NessaConnectionClosedError(1006, "")))
    await expect(source.send(message)).rejects.toMatchObject({ reason: "unavailable" })
    await source.send(message)
    // A later message sends no create: the gateway resolves the conversation itself.
    expect(gateway.count("create")).toBe(0)
    const sends = gateway.calls.filter((call) => call.method === "send")
    const ids = sends.map(
      (call) => call.args[4] as { executionId: string; requestId: string },
    )
    expect(ids.map((sent) => sent.executionId)).toEqual(["m", "m"])
    // A command whose answer was lost reads as unresolved for good, and one
    // refused keeps its refusal: under the same request it could never send.
    expect(ids[0]!.requestId).not.toBe(ids[1]!.requestId)
  })

  it("W3, D12: approving once answers the option the gateway says allows, asks for no read, and the next frame says the conversation", async () => {
    const { gateway, source, updates, follow } = started()
    const asked = permission()
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript(conversationA)
    follow()
    await flush()
    await source.approve(
      conversationA,
      approvalId(reviewEntry(asked)),
      "once",
      "person",
      "opt-a",
    )
    expect(gateway.calls.find((call) => call.method === "answer")?.args).toEqual([
      {
        review: reviewEntry(asked).review,
        conversationId: conversationA,
        executionId: "turn",
        permissionId: "p1",
      },
      "opt-a",
    ])
    expect(gateway.count("subscribe")).toBe(1)
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "2", messages: [running()] }),
    )
    gateway.publish(conversationA)
    await flush()
    // Its first count listed the review loading, its second showed it read.
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ approvals: [], revision: 3 }),
    })
  })

  it("a 100 KB Write review is listed as loading, offers no answer, and is approved once it has loaded whole", async () => {
    const { gateway, source, updates, follow } = started()
    const whole = JSON.stringify({
      file_path: "/tmp/big.txt",
      content: "x".repeat(100_000),
    })
    const asked = permission({ toolName: "Write", argumentsJson: whole })
    let release = () => {}
    const loading = new Promise<void>((resolve) => (release = resolve))
    gateway.once("permissionReview", (normal) => loading.then(normal))
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "1", messages: [running()], permissions: [asked] }),
    )
    follow()
    const shown = await source.transcript(conversationA)
    expect(shown.approvals[0]).toMatchObject({ command: "Write", loading: "reading" })
    // Nothing is answered before the person could see all of what it allows.
    await expect(
      source.approve(
        conversationA,
        approvalId(reviewEntry(asked)),
        "once",
        "person",
        "opt-a",
      ),
    ).rejects.toMatchObject({ reason: "not-supported" })
    expect(gateway.count("answer")).toBe(0)
    release()
    await flush()
    const loaded = transcriptsOf(updates, conversationA).at(-1)
    expect(loaded?.approvals[0]?.command).toBe(`Write ${whole}`)
    expect(loaded?.approvals[0]?.loading).toBeUndefined()
    expect(gateway.count("permissionReview")).toBe(1)
    await source.approve(
      conversationA,
      approvalId(reviewEntry(asked)),
      "once",
      "person",
      "opt-a",
    )
    expect(gateway.calls.find((call) => call.method === "answer")?.args).toEqual([
      {
        review: reviewEntry(asked).review,
        conversationId: conversationA,
        executionId: "turn",
        permissionId: "p1",
      },
      "opt-a",
    ])
  })

  it("a review that could not be read says so, and is read again at once when asked", async () => {
    const { gateway, source, updates, follow } = started()
    const asked = permission()
    gateway.once("permissionReview", async () => {
      throw new NessaRpcError("unavailable", "The gateway could not be reached")
    })
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "1", messages: [running()], permissions: [asked] }),
    )
    follow()
    await source.transcript(conversationA)
    await flush()
    expect(transcriptsOf(updates, conversationA).at(-1)?.approvals[0]).toMatchObject({
      loading: "failed",
    })
    await expect(
      source.approve(
        conversationA,
        approvalId(reviewEntry(asked)),
        "once",
        "person",
        "opt-a",
      ),
    ).rejects.toMatchObject({ reason: "not-supported" })
    source.retryReview?.(conversationA, approvalId(reviewEntry(asked)))
    await flush()
    expect(gateway.count("permissionReview")).toBe(2)
    const loaded = transcriptsOf(updates, conversationA).at(-1)
    expect(loaded?.approvals[0]?.loading).toBeUndefined()
    await source.approve(
      conversationA,
      approvalId(reviewEntry(asked)),
      "once",
      "person",
      "opt-a",
    )
    expect(gateway.count("answer")).toBe(1)
    source.dispose()
  })

  it("W3: approving one of two allow options answers that option", async () => {
    const { gateway, source } = started()
    const asked = permission({
      options: [
        { id: "first", label: "Ship", effect: "allow" },
        { id: "second", label: "Run", effect: "allow" },
      ],
    })
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript(conversationA)
    await source.approve(
      conversationA,
      approvalId(reviewEntry(asked)),
      "once",
      "person",
      "second",
    )
    expect(gateway.calls.find((call) => call.method === "answer")?.args[1]).toBe("second")
  })

  it("W3: approving always is not supported, and nothing is answered", async () => {
    const { gateway, source } = started()
    await expect(
      source.approve(
        conversationA,
        approvalId(reviewEntry(permission())),
        "always",
        "person",
        "opt-a",
      ),
    ).rejects.toMatchObject({ reason: "not-supported" })
    expect(gateway.calls).toEqual([])
  })

  it("W3: approving the project allow sends that option", async () => {
    const { gateway, source } = started()
    const asked = permission({
      options: [
        { id: "opt-a", label: "Allow", effect: "allow" },
        {
          id: "nessa.remember.allow",
          label: "Always allow in this project",
          effect: "allow",
        },
      ],
    })
    gateway.views.set(
      "a",
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript("a")
    await source.approve(
      "a",
      approvalId(reviewEntry(asked)),
      "always",
      "person",
      "nessa.remember.allow",
    )
    expect(gateway.calls.find((call) => call.method === "answer")?.args[1]).toBe(
      "nessa.remember.allow",
    )
  })

  it("W4: denying answers the option the gateway says denies; none offered is not supported", async () => {
    const { gateway, source } = started()
    const asked = permission()
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript(conversationA)
    await flush()
    await source.deny(conversationA, approvalId(reviewEntry(asked)), "person", "opt-b")
    expect(gateway.calls.find((call) => call.method === "answer")?.args[1]).toBe("opt-b")
    const allowOnly = permission({
      permissionId: "p2",
      options: [{ id: "only", label: "Allow", effect: "allow" }],
    })
    gateway.views.set(
      conversationA,
      view(conversationA, {
        revision: "2",
        messages: [running()],
        permissions: [allowOnly],
      }),
    )
    gateway.publish(conversationA)
    await flush()
    await expect(
      source.deny(conversationA, approvalId(reviewEntry(allowOnly)), "person", "only"),
    ).rejects.toMatchObject({
      reason: "not-supported",
    })
    expect(gateway.count("answer")).toBe(1)
  })

  it("W5: an approval no longer asked, or an id this source never wrote, is not waiting", async () => {
    const { gateway, source } = started()
    gateway.views.set(conversationA, view(conversationA, { messages: [running()] }))
    await expect(
      source.approve(
        conversationA,
        approvalId(reviewEntry(permission())),
        "once",
        "person",
        "opt-a",
      ),
    ).rejects.toMatchObject({ reason: "not-waiting" })
    await expect(
      source.deny(conversationA, "not-an-id", "person", "opt-b"),
    ).rejects.toMatchObject({
      reason: "not-waiting",
    })
    expect(gateway.count("answer")).toBe(0)
  })

  it("an approval of a review this window never read is refused, and nothing reads it then", async () => {
    const { gateway, source } = started()
    const asked = permission()
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await expect(
      source.approve(
        conversationA,
        approvalId(reviewEntry(asked)),
        "once",
        "person",
        "opt-a",
      ),
    ).rejects.toMatchObject({ reason: "not-waiting" })
    expect(gateway.count("permissionReview")).toBe(0)
    expect(gateway.count("answer")).toBe(0)
  })

  it("a question asked again under the same ids is answered only by its own key, once read", async () => {
    const { gateway, source } = started()
    const first = {
      executionId: "turn",
      questionId: "q1",
      message: "Where should this land?",
      questions: [
        {
          key: "where",
          prompt: "Pick a place",
          multiSelect: false,
          freeText: false,
          required: true,
          options: [{ value: "here", label: "Here" }],
        },
      ],
    }
    gateway.views.set("a", view("a", { messages: [running()], questions: [first] }))
    await source.transcript("a")
    await flush()
    // After a restart the agent asks again under the same ids: another question.
    const again = { ...first, question: "d".repeat(32), message: "Where now?" }
    let release = () => {}
    const reading = new Promise<void>((resolve) => (release = resolve))
    gateway.once("questionReview", (normal) => reading.then(normal))
    gateway.views.set("a", view("a", { messages: [running()], questions: [again] }))
    gateway.publish("a")
    await flush()
    const choices = [{ key: "where", values: ["here"] }]
    // The card the person saw names the first: stale, and nothing is answered.
    await expect(
      source.answerQuestion("a", "turn", "q1", choices, "person", questionKey(first)),
    ).rejects.toMatchObject({ reason: "not-waiting" })
    // The second, not read yet, offers no answer either.
    await expect(
      source.answerQuestion("a", "turn", "q1", choices, "person", again.question),
    ).rejects.toMatchObject({ reason: "not-waiting" })
    expect(gateway.count("answerQuestion")).toBe(0)
    release()
    await flush()
    await source.answerQuestion("a", "turn", "q1", choices, "person", again.question)
    expect(
      gateway.calls.find((call) => call.method === "answerQuestion")?.args[0],
    ).toMatchObject({ question: again.question, message: "Where now?" })
  })

  it("D21: an approval in a conversation let go past the limit follows it again before answering", async () => {
    const { gateway, source } = started()
    const asked = permission()
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript(conversationA)
    for (let i = 0; i < 8; i++) {
      gateway.views.set(`o${i}`, view(`o${i}`))
      await source.transcript(`o${i}`)
    }
    expect(gateway.live(conversationA)).toBe(0)
    await source.approve(
      conversationA,
      approvalId(reviewEntry(asked)),
      "once",
      "person",
      "opt-a",
    )
    expect(subscribesOf(gateway, conversationA)).toHaveLength(2)
    expect(gateway.count("answer")).toBe(1)
  })

  it("W6: archiving says the removal, at the summary's next revision, before it resolves, and lets its subscription go", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    await source.index()
    await source.transcript(conversationA)
    follow()
    updates.length = 0
    await source.archive(conversationA, "person")
    expect(updates).toEqual([
      { kind: "session-removed", sessionId: conversationA, revision: 2 },
    ])
    expect((await source.index()).sessions).toEqual([])
    await flush()
    expect(gateway.live(conversationA)).toBe(0)
  })

  it("W6: a list frame that arrives while an archive is on its way cannot list the session again after it", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.rows.set(conversationB, row(conversationB))
    follow()
    await source.index()
    updates.length = 0
    const archived = deferred<void>()
    gateway.once("archive", (normal) => archived.promise.then(normal))
    const archiving = source.archive(conversationA, "person")
    await flush()
    // Read before the archive took effect: it still names conversationA.
    gateway.rows.set(conversationB, row(conversationB, { preview: "moved" }))
    gateway.publishList()
    await flush()
    archived.resolve()
    await archiving
    await flush()
    expect(updates).toEqual([
      { kind: "session-removed", sessionId: conversationA, revision: 2 },
      {
        kind: "session",
        session: expect.objectContaining({ id: conversationB, preview: "moved" }),
      },
    ])
    expect((await source.index()).sessions.map((session) => session.id)).toEqual([
      conversationB,
    ])
  })

  it("a rename is the gateway's: the summary says the new title, at its next revision, before it resolves", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    follow()
    await source.index()
    updates.length = 0
    await source.rename(conversationA, "Rain plan", "person")
    expect(gateway.calls).toContainEqual({
      method: "rename",
      args: [conversationA, "Rain plan"],
    })
    expect(updates).toEqual([
      {
        kind: "session",
        session: expect.objectContaining({
          id: conversationA,
          title: "Rain plan",
          revision: 2,
        }),
      },
    ])
    // The list frame that follows says the same, and nothing more.
    gateway.publishList()
    await flush()
    expect(updates).toHaveLength(1)
  })

  it("a list frame read before a rename was stored does not say the old title after it", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    follow()
    await source.index()
    updates.length = 0
    const renamed = deferred<void>()
    gateway.once("rename", (normal) => renamed.promise.then(normal))
    const renaming = source.rename(conversationA, "Rain plan", "person")
    await flush()
    // Read before the rename is stored: it still carries the old title.
    gateway.publishList()
    await flush()
    renamed.resolve()
    await renaming
    await flush()
    const titles = updates.flatMap((update) =>
      update.kind === "session" && update.session.id === conversationA
        ? [update.session.title]
        : [],
    )
    expect(titles.at(-1)).toBe("Rain plan")
    expect(titles.slice(titles.indexOf("Rain plan"))).toEqual(["Rain plan"])
  })

  it("a rename back to the title it had, asked while the first is on its way, is asked too", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    follow()
    await source.index()
    const before = (await source.index()).sessions.find(
      (session) => session.id === conversationA,
    )?.title
    updates.length = 0
    const renamed = deferred<void>()
    gateway.once("rename", (normal) => renamed.promise.then(normal))
    const first = source.rename(conversationA, "Rain plan", "person")
    await flush()
    const back = source.rename(conversationA, before ?? "", "person")
    await flush()
    renamed.resolve()
    await Promise.all([first, back])
    await flush()
    expect(gateway.count("rename")).toBe(2)
    const titles = updates.flatMap((update) =>
      update.kind === "session" && update.session.id === conversationA
        ? [update.session.title]
        : [],
    )
    expect(titles.at(-1)).toBe(before)
  })

  it("D18: a walk owed again keeps what was on its way as its frame came", async () => {
    const { gateway, source, updates, follow, advance } = started()
    for (const id of [conversationA, conversationB, "c", "d"])
      gateway.rows.set(id, row(id, { updatedAtMs: id.charCodeAt(0) * 1_000 }))
    gateway.listLimit = 2
    gateway.observePageSize = 10
    follow()
    await source.index()
    // Listed first of all, so every frame read before its archive names it.
    const newest = [...gateway.rows.values()].sort(
      (one, other) => other.updatedAtMs - one.updatedAtMs,
    )[0].conversationId
    const archived = deferred<void>()
    gateway.once("archive", (normal) => archived.promise.then(normal))
    const archiving = source.archive(newest, "person")
    await flush()
    // A frame read before the archive was stored, whose walk fails.
    gateway.once("observe", () => Promise.reject(rpcCode("unavailable")))
    gateway.publishList()
    await flush()
    archived.resolve()
    await archiving
    await flush()
    updates.length = 0
    // Walked again once the archive is stored: a page that cannot resume
    // keeps the frame's rows, which still name it.
    gateway.once("observe", () => Promise.resolve({ conversations: [], complete: false }))
    await advance(timing.retryMs)
    await flush()
    expect(
      updates.some((update) => update.kind === "session" && update.session.id === newest),
    ).toBe(false)
    expect((await source.index()).sessions.map((session) => session.id)).not.toContain(
      newest,
    )
  })

  it("D18: a walk owed again after a later rename says the later title", async () => {
    const { gateway, source, updates, follow, advance } = started()
    for (const id of [conversationA, conversationB, "c", "d"])
      gateway.rows.set(id, row(id, { updatedAtMs: id.charCodeAt(0) * 1_000 }))
    gateway.listLimit = 2
    gateway.observePageSize = 10
    follow()
    await source.index()
    const newest = [...gateway.rows.values()].sort(
      (one, other) => other.updatedAtMs - one.updatedAtMs,
    )[0].conversationId
    // A frame read before the first rename was stored, whose walk fails.
    const renamed = deferred<void>()
    gateway.once("rename", (normal) => renamed.promise.then(normal))
    const first = source.rename(newest, "Rain plan", "person")
    await flush()
    gateway.once("observe", () => Promise.reject(rpcCode("unavailable")))
    gateway.publishList()
    await flush()
    renamed.resolve()
    await first
    await flush()
    // A second rename, stored while that walk is still owed.
    const second = deferred<void>()
    gateway.once("rename", (normal) => second.promise.then(normal))
    const later = source.rename(newest, "Snow plan", "person")
    await flush()
    second.resolve()
    await later
    await flush()
    updates.length = 0
    gateway.once("observe", () => Promise.resolve({ conversations: [], complete: false }))
    await advance(timing.retryMs)
    await flush()
    const titles = updates.flatMap((update) =>
      update.kind === "session" && update.session.id === newest
        ? [update.session.title]
        : [],
    )
    expect(titles.filter((title) => title !== "Snow plan")).toEqual([])
    expect(
      (await source.index()).sessions.find((session) => session.id === newest)?.title,
    ).toBe("Snow plan")
  })

  it("a rename of a session taken out is refused as unknown and asks nobody", async () => {
    const { gateway, source, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    follow()
    await source.index()
    await source.archive(conversationA, "person")
    await expect(
      source.rename(conversationA, "Rain plan", "person"),
    ).rejects.toMatchObject({
      reason: "unknown-session",
    })
    expect(gateway.count("rename")).toBe(0)
  })

  it("a delete says the removal before it resolves, as an archive does, and a list frame on its way cannot list it again", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.rows.set(conversationB, row(conversationB))
    follow()
    await source.index()
    updates.length = 0
    const deleted = deferred<void>()
    gateway.once("delete", (normal) => deleted.promise.then(normal))
    const deleting = source.delete(conversationA, "person")
    await flush()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.publishList()
    await flush()
    deleted.resolve()
    await deleting
    await flush()
    expect(updates).toContainEqual({
      kind: "session-removed",
      sessionId: conversationA,
      revision: 2,
    })
    expect((await source.index()).sessions.map((session) => session.id)).toEqual([
      conversationB,
    ])
  })

  it("W7: marking read asks nothing of the gateway; pinning is not supported", async () => {
    const { gateway, source } = started()
    await source.markRead(conversationA)
    await expect(source.setPinned(conversationA, true, "person")).rejects.toMatchObject({
      reason: "not-supported",
    })
    expect(gateway.calls).toEqual([])
  })

  it("C5: an archive that never answers settles, and the list frames behind it still apply", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set(conversationA, row(conversationA))
    follow()
    await source.index()
    gateway.once("archive", () => new Promise(() => {}))
    const archived = source
      .archive(conversationA, "person")
      .catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await archived).toMatchObject({ reason: "unavailable" })
    gateway.rows.set(conversationA, row(conversationA, { preview: "after" }))
    gateway.publishList()
    await flush()
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({ id: conversationA, preview: "after" }),
    })
  })

  it("W6b: a second archive of a session is refused as unknown, asks nobody, and says no second removal", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA))
    await source.index()
    follow()
    const outcomes = await Promise.allSettled([
      source.archive(conversationA, "person"),
      source.archive(conversationA, "agent"),
    ])
    expect(outcomes[0].status).toBe("fulfilled")
    expect(outcomes[1]).toMatchObject({
      status: "rejected",
      reason: { reason: "unknown-session" },
    })
    expect(gateway.count("archive")).toBe(1)
    expect(kinds(updates)).toEqual(["session-removed"])
  })

  it("W8: a message asking another model than the one the gateway says it runs is refused, and nothing is sent", async () => {
    const { gateway, source } = started()
    const runs = composerModels[composerModels.length - 1]
    const other = composerModels.find(
      (each) => each.modelId !== runs.modelId || each.provider !== runs.provider,
    )!
    gateway.views.set(
      conversationS,
      view(conversationS, { runtime: runtime(runs.modelId) }),
    )
    await source.transcript(conversationS)
    const asking = (messageId: string, chosen: { provider: string; modelId: string }) =>
      source.send({
        sessionId: conversationS,
        messageId,
        text: "Again",
        model: { provider: chosen.provider, modelId: chosen.modelId },
        initiator: "person",
      })
    await expect(asking("m1", other)).rejects.toMatchObject({ reason: "not-supported" })
    // Carrying `start` changes nothing: only the gateway's word counts.
    await expect(
      source.send({
        sessionId: conversationS,
        messageId: "m2",
        text: "Again",
        model: { provider: other.provider, modelId: other.modelId },
        initiator: "person",
        start: { channelId: "gateway-conversations", title: "Again" },
      }),
    ).rejects.toMatchObject({ reason: "not-supported" })
    expect(gateway.count("send")).toBe(0)
    await asking("m3", runs)
    expect(gateway.count("send")).toBe(1)
  })

  it("W8: a first message and a resend on another model, before any frame, record nothing the gateway did not say", async () => {
    const { gateway, source } = started()
    const start = { channelId: "gateway-conversations", title: "Hi" }
    // The first message's subscription is still on its way when the resend goes.
    const held = deferred<void>()
    gateway.once("subscribe", (normal) => held.promise.then(normal))
    await source.send({
      sessionId: conversationS,
      messageId: "m1",
      text: "Hi",
      model,
      initiator: "person",
      start,
    })
    const other = { provider: "openai", modelId: "gpt-5" }
    // Not said yet: it cannot be told, so it goes — and it runs on what the gateway created.
    await source.send({
      sessionId: conversationS,
      messageId: "m2",
      text: "Hi",
      model: other,
      initiator: "person",
      start,
    })
    // Each first message opens (`create` reopens, keeping the model it was created with).
    expect(gateway.count("create")).toBe(2)
    expect(gateway.count("send")).toBe(2)
    held.resolve()
  })

  it("W8: an unknown model cannot be compared, so the message goes", async () => {
    const { gateway, source } = started()
    gateway.views.set(conversationA, view(conversationA))
    await source.transcript(conversationA)
    await source.send({
      sessionId: conversationA,
      messageId: "m",
      text: "Hi",
      model: { provider: "openai", modelId: "gpt-5" },
      initiator: "person",
    })
    expect(gateway.count("send")).toBe(1)
  })

  it("W8: the model is checked at the send, after anything the call waited on", async () => {
    const { gateway, source } = started()
    const runs = composerModels[composerModels.length - 1]
    const other = composerModels.find(
      (each) => each.modelId !== runs.modelId || each.provider !== runs.provider,
    )!
    gateway.views.set(
      conversationS,
      view(conversationS, { runtime: runtime(runs.modelId) }),
    )
    // While the first message's create is on its way, a frame says what the conversation runs.
    gateway.once("create", async (normal) => {
      await source.transcript(conversationS)
      return normal()
    })
    await expect(
      source.send({
        sessionId: conversationS,
        messageId: "m",
        text: "Hi",
        model: { provider: other.provider, modelId: other.modelId },
        initiator: "person",
        start: { channelId: "gateway-conversations", title: "Hi" },
      }),
    ).rejects.toMatchObject({ reason: "not-supported" })
    expect(gateway.count("send")).toBe(0)
  })

  it("F5: a message the client refuses before sending is not supported, not a fault", async () => {
    const { gateway, source } = started()
    await expect(
      source.send({
        sessionId: conversationA,
        messageId: "m",
        text: "x".repeat(9000),
        model,
        initiator: "person",
      }),
    ).rejects.toMatchObject({ reason: "not-supported" })
    expect(gateway.count("send")).toBe(0)
  })

  it("C6: an approval whose call settled unavailable is never sent, so the person's later answer is the one taken", async () => {
    const { gateway, source, advance } = started()
    const asked = permission()
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "1", messages: [running()], permissions: [asked] }),
    )
    // Shown, then let go past the limit, so the approval follows it again.
    await source.transcript(conversationA)
    await flush()
    for (let i = 0; i < 8; i++) {
      gateway.views.set(`o${i}`, view(`o${i}`))
      await source.transcript(`o${i}`)
    }
    // The conversation's subscription answers only after the approval's call has settled.
    const held = deferred<void>()
    gateway.once("subscribe", (normal) => held.promise.then(normal))
    const approving = source
      .approve(conversationA, approvalId(reviewEntry(asked)), "once", "person", "opt-a")
      .catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await approving).toMatchObject({ reason: "unavailable" })
    held.resolve()
    await flush()
    expect(gateway.count("answer")).toBe(0)
    // The person answers again, and that is the answer taken.
    await source.deny(conversationA, approvalId(reviewEntry(asked)), "person", "opt-b")
    expect(
      gateway.calls
        .filter((call) => call.method === "answer")
        .map((call) => call.args[1]),
    ).toEqual(["opt-b"])
  })

  it("C6: an archive whose call settled unavailable is never sent", async () => {
    const { gateway, source, follow, advance } = started()
    for (const id of [conversationA, conversationB])
      gateway.rows.set(id, row(id, { updatedAtMs: id === conversationA ? 1 : 2 }))
    gateway.listLimit = 1
    // Two incomplete list frames ahead of the archive, each walking observe:
    // the first walk never answers; the second starts when it times out and
    // answers after the archive's call has settled.
    gateway.once("observe", () => new Promise(() => {}))
    const second = deferred<void>()
    gateway.once("observe", (normal) => second.promise.then(normal))
    follow()
    await flush()
    gateway.rows.set("c", row("c", { updatedAtMs: 3 }))
    gateway.publishList()
    await advance(1_000)
    const archiving = source
      .archive(conversationA, "person")
      .catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await archiving).toMatchObject({ reason: "unavailable" })
    second.resolve()
    await flush()
    expect(gateway.count("observe")).toBe(2)
    expect(gateway.count("archive")).toBe(0)
  })

  it("C6: a first message whose call settled before the gateway answered sends neither create nor message", async () => {
    const gateway = fakeGateway()
    const arriving = deferred<FakeGateway["client"]>()
    const { source, advance } = started(gateway, () => arriving.promise)
    void source.index().catch(() => undefined)
    const sending = source
      .send({
        sessionId: conversationS,
        messageId: "m",
        text: "Hi",
        model,
        initiator: "person",
        start: { channelId: "gateway-conversations", title: "Hi" },
      })
      .catch((error: unknown) => error)
    await advance(timing.callMs - 1)
    expect(await Promise.race([sending, Promise.resolve("pending")])).toBe("pending")
    // Connected just in time for the connection, too late for this message's call.
    const ready = gateway.client
    await advance(1)
    arriving.resolve(ready)
    await flush()
    expect(await sending).toMatchObject({ reason: "unavailable" })
    expect(gateway.count("create")).toBe(0)
    expect(gateway.count("send")).toBe(0)
  })

  it("C6: a message whose create answered after its call settled is not sent", async () => {
    const { gateway, source, advance } = started()
    const created = deferred<unknown>()
    gateway.once("create", () => created.promise)
    const sending = source
      .send({
        sessionId: conversationS,
        messageId: "m",
        text: "Hi",
        model,
        initiator: "person",
        start: { channelId: "gateway-conversations", title: "Hi" },
      })
      .catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await sending).toMatchObject({ reason: "unavailable" })
    created.resolve({ conversationId: conversationS })
    await flush()
    expect(gateway.count("send")).toBe(0)
  })

  it("R8: a session taken out is not followed, not sent to, and not opened by the subscriptions", async () => {
    const { gateway, source, follow, advance } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA, { messages: [running()] }))
    await source.index()
    await source.archive(conversationA, "person")
    await expect(source.transcript(conversationA)).rejects.toMatchObject({
      reason: "unknown-session",
    })
    await expect(
      source.send({
        sessionId: conversationA,
        messageId: "m",
        text: "Hi",
        model,
        initiator: "person",
      }),
    ).rejects.toMatchObject({ reason: "unknown-session" })
    follow()
    await advance(timing.retryMs * 3)
    expect(gateway.count("subscribe")).toBe(0)
    expect(gateway.count("send")).toBe(0)
  })

  it("R8, W6b: a session archived before any list named it is remembered as taken out", async () => {
    const { gateway, source } = started()
    const start = { channelId: "gateway-conversations", title: "Hi" }
    await source.send({
      sessionId: conversationS,
      messageId: "m1",
      text: "Hi",
      model,
      initiator: "person",
      start,
    })
    await source.archive(conversationS, "person")
    await expect(source.archive(conversationS, "person")).rejects.toMatchObject({
      reason: "unknown-session",
    })
    await expect(
      source.send({
        sessionId: conversationS,
        messageId: "m2",
        text: "Hi",
        model,
        initiator: "person",
        start,
      }),
    ).rejects.toMatchObject({ reason: "unknown-session" })
    expect(gateway.count("archive")).toBe(1)
    expect(gateway.count("create")).toBe(1)
  })

  it("S3b: a session listed again after its removal is not said to wait on the person from its old frame", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set(conversationA, row(conversationA, { running: true }))
    gateway.views.set(
      conversationA,
      view(conversationA, { messages: [running()], permissions: [permission()] }),
    )
    follow()
    await source.index()
    await source.transcript(conversationA)
    gateway.rows.delete(conversationA)
    gateway.publishList()
    await flush()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.publishList()
    await flush()
    const relisted = updates.filter((update) => update.kind === "session").pop()
    expect(relisted).toMatchObject({ session: { id: conversationA, status: "idle" } })
  })
})

describe("refusals are typed (F)", () => {
  const rpc = (code: string) => new NessaRpcError(code, "message text nobody parses")

  it("F1: a deleted conversation, or one not found, is an unknown session", () => {
    expect(refusalOf(rpc("conversation_deleted"))).toMatchObject({
      reason: "unknown-session",
    })
    expect(
      refusalOf(
        new NessaConversationMutationError(
          "c",
          "r",
          "e",
          rpc("conversation_not_found"),
          () => Promise.resolve(),
        ),
      ),
    ).toMatchObject({ reason: "unknown-session" })
  })

  it("F2: a stale permission is not waiting", () => {
    expect(
      refusalOf(
        new NessaConversationControlError("c", "r", "e", rpc("stale_permission"), true),
      ),
    ).toMatchObject({ reason: "not-waiting" })
  })

  // A bare NessaRpcError comes only from a subscription or an observe page,
  // which the client wraps in nothing: certain, since a read takes no effect.
  it("F3: a subscription refused for good is not supported", () => {
    expect(refusalOf(rpc("agent_not_configured"))).toMatchObject({
      reason: "agent-not-configured",
      diagnostic: { code: "agent_not_configured" },
    })
    for (const code of [
      "agent_unsupported",
      "conversations_not_configured",
      "model_unavailable",
      "invalid_request",
      "submission_conflict",
      "conversation_state_unreadable",
      "conversation_configuration_changed",
    ])
      expect(refusalOf(rpc(code))).toMatchObject({ reason: "not-supported" })
  })

  it("F3: a code for what may yet be done, an unknown one, or no answer is unavailable", () => {
    for (const error of [
      rpc("temporarily_unavailable"),
      rpc("conversation_capacity"),
      rpc("submission_unresolved"),
      rpc("subscription_capacity"),
      rpc("a_code_nobody_taught_this_build"),
      new NessaConnectionClosedError(1006, ""),
      new NessaConversationControlError("c", "r", "e", new Error("lost"), false),
    ])
      expect(refusalOf(error)).toMatchObject({ reason: "unavailable" })
  })

  it("F3: a fault that is no answer is passed on as it is, for the window to log", async () => {
    const { gateway, source } = started()
    const fault = new Error("Invalid conversation response")
    gateway.once("subscribeList", () => Promise.reject(fault))
    await expect(source.index()).rejects.toBe(fault)
  })

  it("F4: a refusal for good the client cannot say was refused before it ran may have been done", () => {
    // submission_conflict is not certain before dispatch, by the client's own table.
    expect(
      refusalOf(
        new NessaConversationMutationError(
          "c",
          "r",
          "e",
          rpc("submission_conflict"),
          () => Promise.resolve(),
        ),
      ),
    ).toMatchObject({ reason: "unavailable" })
    // agent_not_configured is: then it is for good.
    expect(
      refusalOf(
        new NessaConversationMutationError(
          "c",
          "r",
          "e",
          rpc("agent_not_configured"),
          () => Promise.resolve(),
        ),
      ),
    ).toMatchObject({ reason: "agent-not-configured" })
    // Where the target stands holds whatever the command did.
    expect(
      refusalOf(
        new NessaConversationControlError("c", "r", "e", rpc("stale_permission"), true),
      ),
    ).toMatchObject({ reason: "not-waiting" })
  })

  it("traces a refused subscription with the session, the subject, and the gateway code", async () => {
    const debug = vi.spyOn(console, "debug").mockImplementation(() => {})
    const warn = quiet()
    try {
      const { gateway, source } = started()
      gateway.once("subscribe", () => Promise.reject(rpcCode("temporarily_unavailable")))
      await expect(source.transcript(conversationA)).rejects.toMatchObject({
        reason: "unavailable",
      })
      expect(debug).toHaveBeenCalledWith("[nessa] conversation read asked", {
        subject: "conversation",
        method: "conversation.subscribe",
        sessionId: conversationA,
      })
      expect(warn).toHaveBeenCalledWith("[nessa] conversation read refused", {
        subject: "conversation",
        method: "conversation.subscribe",
        sessionId: conversationA,
        reason: "unavailable",
        code: "temporarily_unavailable",
      })
      expect(JSON.stringify(warn.mock.calls)).not.toContain("message text nobody parses")

      debug.mockClear()
      warn.mockClear()
      gateway.once("subscribe", () =>
        Promise.reject(new NessaConnectionClosedError(1001, "going away")),
      )
      await expect(source.transcript(conversationA)).rejects.toMatchObject({
        reason: "unavailable",
      })
      expect(warn).toHaveBeenCalledWith("[nessa] conversation read refused", {
        subject: "conversation",
        method: "conversation.subscribe",
        sessionId: conversationA,
        reason: "unavailable",
        code: "1001",
        closeReason: "transport_interrupted",
        socket: "closed",
      })
      expect(JSON.stringify(warn.mock.calls)).not.toContain("going away")

      debug.mockClear()
      warn.mockClear()
      gateway.once("subscribeList", () =>
        Promise.reject(rpcCode("temporarily_unavailable")),
      )
      await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
      expect(debug).toHaveBeenCalledWith("[nessa] conversation read asked", {
        subject: "index",
        method: "conversation.subscribeList",
      })
      expect(warn).toHaveBeenCalledWith("[nessa] conversation read refused", {
        subject: "index",
        method: "conversation.subscribeList",
        reason: "unavailable",
        code: "temporarily_unavailable",
      })
    } finally {
      debug.mockRestore()
      warn.mockRestore()
    }
  })
})

describe("through the window's store, unchanged", () => {
  it("opens on the gateway's conversations, shows one, follows it, and resyncs on reconnect", async () => {
    const { gateway, source } = started()
    gateway.rows.set(conversationA, row(conversationA, { running: true }))
    gateway.views.set(
      conversationA,
      view(conversationA, {
        revision: "1",
        messages: [
          {
            ...running("turn"),
            parts: [
              {
                offset: 0,
                kind: "text",
                text: "Working on it",
                toolId: "",
                noticeId: "",
              },
            ],
          },
        ],
      }),
    )
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    await flush()
    const state = () => store.getState().workspace
    expect(state().status).toBe("ready")
    expect(Object.keys(state().sessions)).toEqual([conversationA])
    expect(state().transcripts[conversationA]?.messages.map(messageText)).toEqual([
      "Run the tests",
      "Working on it",
    ])
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "2", messages: [running("turn")] }),
    )
    gateway.publish(conversationA)
    await flush()
    expect(state().transcripts[conversationA]?.revision).toBe(2)
    const lists = gateway.count("subscribeList")
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    gateway.setState({ status: "connected" })
    await flush()
    expect(gateway.count("subscribeList")).toBe(lists + 1)
    expect(state().status).toBe("ready")
  })

  it("sends a message the window's conversation then shows under its id", async () => {
    const { gateway, source } = started()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    await flush()
    gateway.once("send", async (normal) => {
      const { executionId } = gateway.calls[gateway.calls.length - 1].args[4] as {
        executionId: string
      }
      gateway.rows.set(
        conversationA,
        row(conversationA, { running: true, updatedAtMs: 3_000 }),
      )
      gateway.views.set(
        conversationA,
        view(conversationA, {
          revision: "2",
          messages: [{ ...running(executionId), userText: "Hello there" }],
        }),
      )
      const sent = await normal()
      gateway.publish(conversationA)
      return sent
    })
    const outcome = await store.dispatch(
      sendMessage({ sessionId: conversationA, text: "Hello there", initiator: "person" }),
    )
    expect(outcome).toBe("sent")
    await flush()
    // The gateway's conversation holds it under the id it was sent with: the outbox lets it go.
    expect(store.getState().workspace.outbox[conversationA]).toBeUndefined()
    expect(
      store.getState().workspace.transcripts[conversationA]?.messages.map(messageText),
    ).toEqual(["Hello there"])
  })
})

describe("MCP Apps (#384)", () => {
  /** A source whose apps are a recorder, told each view and each deletion. */
  function withApps(gateway: FakeGateway = fakeGateway()) {
    const { clock, advance } = manualClock()
    const told: (["observe", string, string] | ["forget", string])[] = []
    const source = gatewaySource({
      connect: () => Promise.resolve(gateway.client),
      clock,
      timing,
      apps: {
        observe: (seen) => told.push(["observe", seen.conversationId, seen.revision]),
        forget: (conversationId) => told.push(["forget", conversationId]),
      },
    })
    const updates: WorkspaceUpdate[] = []
    const follow = () => source.subscribe((update) => updates.push(update))
    return { gateway, source, told, updates, follow, advance }
  }

  const appTool = {
    executionId: "turn",
    toolId: "call-1",
    title: "show_chart",
    kind: "other" as const,
    status: "running" as const,
    details: "",
    input: "",
    nativeHooks: "off" as const,
    mcp: { server: "mcptest", tool: "show_chart", resourceUri: "ui://t/chart.html" },
  }

  it("each view applied is told to the apps in the order applied, and the same view is not told twice", async () => {
    const { gateway, source, told } = withApps()
    gateway.views.set(conversationA, view(conversationA, { revision: "1", title: "1" }))
    await source.transcript(conversationA)
    gateway.publish(conversationA)
    await flush()
    gateway.views.set(conversationA, view(conversationA, { revision: "2", title: "2" }))
    gateway.publish(conversationA)
    await flush()
    expect(told).toEqual([
      ["observe", conversationA, "1"],
      ["observe", conversationA, "2"],
    ])
  })

  it("a subscription let go is not told: one whose open outlived its call", async () => {
    const { gateway, source, told, advance } = withApps()
    gateway.views.set(conversationA, view(conversationA, { revision: "late" }))
    const held = deferred<void>()
    gateway.once("subscribe", (normal) => held.promise.then(normal))
    const call = source.transcript(conversationA).catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await call).toMatchObject({ reason: "unavailable" })
    held.resolve()
    await flush()
    expect(told).toEqual([])
    expect(gateway.live(conversationA)).toBe(0)
  })

  it("an app's widget is named in its conversation, so two conversations' calls under one id are two widgets", async () => {
    const { gateway, source } = withApps()
    const tools = [appTool]
    const messages = [
      {
        ...running(),
        parts: [
          { offset: 0, kind: "tool" as const, text: "", toolId: "call-1", noticeId: "" },
        ],
      },
    ]
    gateway.views.set(conversationA, view(conversationA, { tools, messages }))
    gateway.views.set(conversationB, view(conversationB, { tools, messages }))
    const widgetOf = async (id: string) =>
      (await source.transcript(id)).messages
        .flatMap((message) => message.parts)
        .find((part) => part.kind === "widget")
    const [a, b] = [await widgetOf(conversationA), await widgetOf(conversationB)]
    expect(a).toBeDefined()
    expect(b).toBeDefined()
    expect(a).not.toEqual(b)
  })

  it("D4: a conversation the gateway says was deleted is forgotten by the apps, after it is taken out", async () => {
    const { gateway, source, told, updates, follow } = withApps()
    gateway.rows.set(conversationA, row(conversationA))
    gateway.views.set(conversationA, view(conversationA))
    await source.index()
    follow()
    await source.transcript(conversationA)
    gateway.end(conversationA, { reason: "refused", code: "conversation_deleted" })
    await flush()
    expect(kinds(updates)).toContain("session-removed")
    expect(told).toEqual([
      ["observe", conversationA, "r1"],
      ["forget", conversationA],
    ])
  })

  it("a subscription refused as deleted, even as a control error, is forgotten too", async () => {
    const { gateway, source, told } = withApps()
    gateway.once("subscribe", () => Promise.reject(rpcCode("conversation_deleted")))
    await expect(source.transcript(conversationA)).rejects.toMatchObject({
      reason: "unknown-session",
    })
    gateway.once("subscribe", () =>
      Promise.reject(
        new NessaConversationControlError(
          conversationB,
          "r",
          undefined,
          rpcCode("conversation_deleted"),
        ),
      ),
    )
    await source.transcript(conversationB).catch(() => undefined)
    expect(told).toEqual([
      ["forget", conversationA],
      ["forget", conversationB],
    ])
  })

  it("not found, an archive, or a complete list that leaves it out forgets nothing: a closed or archived conversation comes back", async () => {
    const { gateway, source, told, follow } = withApps()
    follow()
    // Not found may be this caller's alone.
    await source.transcript("missing").catch(() => undefined)
    // Archived here.
    gateway.rows.set(conversationB, row(conversationB))
    gateway.publishList()
    await source.index()
    await source.archive(conversationB, "person")
    // Missing from a complete list: archived or deleted elsewhere, which a list cannot tell.
    gateway.rows.set("c", row("c"))
    gateway.publishList()
    await flush()
    gateway.rows.delete("c")
    gateway.publishList()
    await flush()
    expect(told).toEqual([])
  })

  it("the apps' fault is theirs: the frame still applies and is said", async () => {
    const gateway = fakeGateway()
    const { clock } = manualClock()
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const source = gatewaySource({
      connect: () => Promise.resolve(gateway.client),
      clock,
      timing,
      apps: {
        observe: () => {
          throw new Error("app registry broke")
        },
        forget: () => {},
      },
    })
    const updates: WorkspaceUpdate[] = []
    source.subscribe((update) => updates.push(update))
    gateway.views.set(conversationA, view(conversationA))
    await expect(source.transcript(conversationA)).resolves.toMatchObject({
      sessionId: conversationA,
    })
    expect(kinds(updates)).toContain("transcript")
    expect(error).toHaveBeenCalled()
    error.mockRestore()
  })

  it("an app's calls go on the client the source holds, and none once it is disposed", async () => {
    const { gateway, source } = withApps()
    expect(await source.connected()).toBe(gateway.client)
    await source.index()
    expect(await source.connected()).toBe(gateway.client)
    source.dispose()
    await expect(source.connected()).rejects.toMatchObject({ reason: "unavailable" })
  })
})

describe("an app's review is shown after its turn ended (#436)", () => {
  const ended = { ...running(), status: "completed" as const }
  const appReview = permission({
    permissionId: "app-1",
    title: "An app asks to run app_delete_row on mcptest",
    toolName: "app_delete_row",
    origin: { kind: "app", server: "mcptest", tool: "app_delete_row" },
    options: [
      { id: "allow", label: "Allow", effect: "allow" },
      { id: "deny", label: "Deny", effect: "deny" },
    ],
  })

  /** Conversations whose turns have ended, listed and followed by the window, none opened. */
  async function idle(ids: readonly string[] = [conversationA], setup = started()) {
    for (const id of ids) {
      setup.gateway.rows.set(id, row(id))
      setup.gateway.views.set(id, view(id, { revision: "1", messages: [ended] }))
    }
    setup.follow()
    await setup.source.index()
    return setup
  }

  it("P2, P3, D11: an app's call follows its conversation, and the review it opens is shown as the app's", async () => {
    const { gateway, source, updates } = await idle()
    const answer = deferred<string>()
    const call = source.appCall(conversationA, () => answer.promise)
    await flush()
    expect(gateway.live(conversationA)).toBe(1)
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "2", messages: [ended] }),
    )
    gateway.publish(conversationA)
    await flush()
    // The review is a live fact of the view: the fold's revision stays.
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "2", messages: [ended], permissions: [appReview] }),
    )
    gateway.publish(conversationA)
    await flush()
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({
        sessionId: conversationA,
        approvals: [
          {
            id: approvalId(reviewEntry(appReview)),
            command: "app_delete_row {}",
            reason: "An app asks to run app_delete_row on mcptest",
            origin: { kind: "app", server: "mcptest", tool: "app_delete_row" },
            options: [
              { id: "allow", label: "Allow", choice: "once" },
              { id: "deny", label: "Deny", choice: "deny" },
            ],
            ask: "tool",
          },
        ],
      }),
    })
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({ id: conversationA, status: "needs-you" }),
    })
    answer.resolve("answered")
    // The call settles as the app's own call did; the conversation stays followed.
    expect(await call).toBe("answered")
    await flush()
    expect(gateway.live(conversationA)).toBe(1)
  })

  it("P4: the person's answer to an app's review goes to the gateway by the option's id", async () => {
    const { gateway, source, updates } = await idle()
    const answer = deferred<string>()
    void source.appCall(conversationA, () => answer.promise)
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "2", messages: [ended], permissions: [appReview] }),
    )
    await flush()
    gateway.onceAnswer(async (normal) => {
      answer.resolve("allowed")
      return normal()
    })
    await source.approve(
      conversationA,
      approvalId(reviewEntry(appReview)),
      "once",
      "person",
      "allow",
    )
    expect(gateway.calls.find((call) => call.method === "answer")?.args).toEqual([
      {
        review: reviewEntry(appReview).review,
        conversationId: conversationA,
        executionId: "turn",
        permissionId: "app-1",
      },
      "allow",
    ])
    gateway.views.set(
      conversationA,
      view(conversationA, { revision: "3", messages: [ended] }),
    )
    gateway.publish(conversationA)
    await flush()
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ approvals: [], revision: 3 }),
    })
  })

  it("P7: a call that is refused, or throws before it is sent, is passed on as it was", async () => {
    const { source } = await idle()
    const refusal = new Error("mcp_tool_not_for_app")
    await expect(
      source.appCall(conversationA, () => Promise.reject(refusal)),
    ).rejects.toBe(refusal)
    const thrown = new TypeError("past the bounds")
    await expect(
      source.appCall(conversationA, () => {
        throw thrown
      }),
    ).rejects.toBe(thrown)
  })

  it("P8: a call in one conversation follows that conversation alone", async () => {
    const { gateway, source } = await idle([conversationA, conversationB])
    const answer = deferred<string>()
    void source.appCall(conversationA, () => answer.promise)
    await flush()
    expect(gateway.live(conversationA)).toBe(1)
    expect(gateway.live(conversationB)).toBe(0)
    answer.resolve("done")
  })

  it("P9: a conversation taken out is not followed for an app's call, which still goes to the gateway", async () => {
    const { gateway, source } = await idle()
    gateway.rows.delete(conversationA)
    gateway.publishList()
    await flush()
    const asked = vi.fn(() => Promise.resolve("answered"))
    await expect(source.appCall(conversationA, asked)).resolves.toBe("answered")
    await flush()
    expect(asked).toHaveBeenCalledTimes(1)
    expect(gateway.count("subscribe")).toBe(0)
  })
})

// d2dbcd60f2 (#400) retained caller-owned message/model references across create.
it("PS16: captures immutable workspace intent before creation can await", async () => {
  const { gateway, source } = started()
  const created = deferred<void>()
  gateway.once("create", (normal) => created.promise.then(normal))
  const message = {
    sessionId: conversationS as string,
    messageId: "original-turn",
    text: "original",
    model: { ...model },
    initiator: "person" as const,
    start: { channelId: "gateway-conversations", title: "original" },
  }
  const sending = source.send(message)
  await flush()
  expect(gateway.count("create")).toBe(1)
  message.sessionId = conversationB
  message.messageId = "other-turn"
  message.text = "edited"
  message.model.modelId = "other-model"
  message.start = { channelId: "other", title: "edited" }
  created.resolve()
  await sending
  await flush()
  const submission = gateway.calls.find((call) => call.method === "send")!
  expect(submission.args).toEqual([
    conversationS,
    "original",
    [],
    [],
    { executionId: "original-turn", requestId: expect.any(String) },
  ])
  expect(gateway.live(conversationS)).toBe(1)
  expect(gateway.live(conversationB)).toBe(0)
})

it("keeps input attachments when a fresh desktop source reopens a retained conversation", async () => {
  const gateway = fakeGateway()
  const image = { digest: "a".repeat(64), mimeType: "image/png" as const, size: 2048 }
  const file = { path: "/workspace/quarterly [report].csv" }
  gateway.views.set(
    conversationA,
    view(conversationA, {
      messages: [
        {
          executionId: "original-turn",
          userText: "Inspect this.",
          attachments: [image],
          files: [file],
          status: "completed",
          parts: [],
        },
      ],
    }),
  )
  const first = started(gateway)
  const stop = first.follow()
  const original = await first.source.transcript(conversationA)
  stop()
  const reopened = started(gateway)
  const stopReopened = reopened.follow()
  try {
    const restored = await reopened.source.transcript(conversationA)
    expect(restored.sessionId).toBe(conversationA)
    expect(restored.messages[0].parts).toEqual(original.messages[0].parts)
    expect(restored.messages[0].parts).toEqual([
      { kind: "text", text: "Inspect this." },
      {
        kind: "attachments",
        content: [
          { type: "image-reference", ...image },
          { type: "file-reference", ...file },
        ],
      },
    ])
    expect(gateway.count("subscribe")).toBe(2)
  } finally {
    stopReopened()
  }
})
