// @vitest-environment jsdom
/**
 * The gateway source against a fake client, one or more tests per row of the
 * table on #248 (row ids in the test names): connection (C), reads (R), the
 * stream (S), writes (W) and refusals (F).
 */
import {
  NessaConnectionClosedError,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  type ConversationPermission,
} from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import { WorkspaceSourceError, type WorkspaceUpdate } from "../../application/ports"
import { composerModels } from "../../../model/composer-options"
import { messageText } from "../../model/transcript"
import { testStore } from "../../testing"
import { followWorkspace, loadWorkspace, sendMessage } from "../store/commands"
import { approvalId } from "./gateway-views"
import { deferred, fakeGateway, row, view, type FakeGateway } from "./fake-gateway"
import { gatewaySource, refusalOf, type GatewayClock } from "./gateway-source"

const timing = { callMs: 5_000, pollMs: 100 }
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

const kinds = (updates: readonly WorkspaceUpdate[]) =>
  updates.map((update) => update.kind)

const permission = (
  change: Partial<ConversationPermission> = {},
): ConversationPermission => ({
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

describe("reads", () => {
  it("C1, R1: connected, lists every conversation in one section and channel, each at its first revision", async () => {
    const { gateway, source } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.rows.set("b", row("b", { title: null, preview: null }))
    const index = await source.index()
    expect(index.sections).toHaveLength(1)
    expect(index.channels).toEqual([
      expect.objectContaining({ sectionId: index.sections[0].id }),
    ])
    expect(index.sessions).toEqual([
      expect.objectContaining({
        id: "a",
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
        id: "b",
        title: "",
        preview: "",
        status: "idle",
        revision: 1,
      }),
    ])
  })

  it("R1: the same list again keeps every revision", async () => {
    const { gateway, source } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    expect((await source.index()).sessions[0].revision).toBe(1)
  })

  it("R2: an incomplete list keeps the sessions it leaves out; a complete one takes them out", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.rows.set("a", row("a"))
    gateway.rows.set("b", row("b"))
    await source.index()
    gateway.rows.delete("b")
    gateway.complete = false
    expect((await source.index()).sessions.map((session) => session.id)).toEqual([
      "a",
      "b",
    ])
    expect(kinds(updates)).not.toContain("session-removed")
    gateway.complete = true
    expect((await source.index()).sessions.map((session) => session.id)).toEqual(["a"])
    expect(updates).toContainEqual({
      kind: "session-removed",
      sessionId: "b",
      revision: 2,
    })
  })

  it("R3: a read sends no create, and reads at its first revision", async () => {
    const { gateway, source } = started()
    gateway.views.set("a", view("a"))
    const transcript = await source.transcript("a")
    expect(transcript).toMatchObject({ sessionId: "a", revision: 1 })
    expect(gateway.calls.map((call) => call.method)).toEqual(["read"])
  })

  it("R6: the same gateway revision is the same transcript; a new one is the next", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    gateway.views.set("a", view("a", { revision: "x" }))
    expect((await source.transcript("a")).revision).toBe(1)
    expect((await source.transcript("a")).revision).toBe(1)
    expect(kinds(updates).filter((kind) => kind === "transcript")).toHaveLength(1)
    gateway.views.set("a", view("a", { revision: "y" }))
    expect((await source.transcript("a")).revision).toBe(2)
    expect(kinds(updates).filter((kind) => kind === "transcript")).toHaveLength(2)
  })

  it("R4: a second read of one conversation waits for the first, and they apply in order", async () => {
    const { gateway, source } = started()
    gateway.views.set("a", view("a", { revision: "x" }))
    await source.transcript("a")
    const first = deferred<unknown>()
    gateway.once("read", () => first.promise)
    const older = source.transcript("a")
    const newer = source.transcript("a")
    await flush()
    expect(gateway.count("read")).toBe(2)
    gateway.views.set("a", view("a", { revision: "z" }))
    first.resolve(view("a", { revision: "y" }))
    expect((await older).revision).toBe(2)
    expect((await newer).revision).toBe(3)
    expect(gateway.count("read")).toBe(3)
  })

  it("R5: a read answered after its call timed out is let go", async () => {
    const { gateway, source, advance } = started()
    gateway.views.set("a", view("a", { revision: "new" }))
    const late = deferred<unknown>()
    gateway.once("read", () => late.promise)
    const timedOut = source.transcript("a")
    const outcome = timedOut.catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await outcome).toMatchObject({ reason: "unavailable" })
    expect((await source.transcript("a")).revision).toBe(1)
    late.resolve(view("a", { revision: "old" }))
    await flush()
    // The newer read stands: the late, older answer moved nothing.
    const again = await source.transcript("a")
    expect(again.revision).toBe(1)
  })
})

describe("every call settles on its own timer (C4)", () => {
  it("a list, a read, a send and an archive that get no answer settle unavailable", async () => {
    const { gateway, source, advance } = started()
    gateway.views.set("a", view("a"))
    await source.transcript("a")
    for (const method of ["list", "read", "send", "archive"] as const)
      gateway.once(method, () => new Promise(() => {}))
    const calls = [
      source.index(),
      source.transcript("a"),
      source.send({
        sessionId: "a",
        messageId: "m",
        text: "hi",
        model,
        initiator: "person",
      }),
      source.archive("a", "person"),
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
    expect(late.count("list")).toBe(0)
  })

  it("a connection that fails is unavailable, and the next call connects again", async () => {
    const gateway = fakeGateway()
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
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

describe("the connection", () => {
  it("C2: says resync when the client reconnects", async () => {
    const { gateway, source, updates, follow } = started()
    follow()
    await source.index()
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    expect(updates).toEqual([])
    gateway.setState({ status: "connected" })
    expect(updates).toEqual([{ kind: "resync" }])
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
    await source.index()
    expect(attempts).toBe(2)
    expect(updates).toContainEqual({ kind: "resync" })
    expect(next.count("list")).toBe(1)
  })

  it("C3: dispose closes the client, stops polling, refuses later calls and says nothing more", async () => {
    const { gateway, source, updates, follow, advance } = started()
    follow()
    gateway.rows.set("a", row("a"))
    await source.index()
    const lists = gateway.count("list")
    source.dispose()
    expect(gateway.closed()).toBe(true)
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    await advance(timing.pollMs * 5)
    expect(gateway.count("list")).toBe(lists)
    const said = updates.length
    gateway.setState({ status: "connected" })
    expect(updates).toHaveLength(said)
  })

  it("C3: a call in flight when the source is disposed settles unavailable", async () => {
    const { gateway, source } = started()
    const held = deferred<unknown>()
    gateway.once("list", () => held.promise)
    const index = source.index()
    await flush()
    source.dispose()
    held.resolve({ conversations: [], complete: true })
    await expect(index).rejects.toMatchObject({ reason: "unavailable" })
  })
})

describe("the stream", () => {
  it("S1: polls while followed, saying a changed summary at its next revision and an unchanged one not at all", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    follow()
    await advance(timing.pollMs)
    expect(updates).toEqual([])
    gateway.rows.set("a", row("a", { preview: "Something new", updatedAtMs: 3_000 }))
    await advance(timing.pollMs)
    expect(updates).toEqual([
      {
        kind: "session",
        session: expect.objectContaining({
          id: "a",
          preview: "Something new",
          revision: 2,
        }),
      },
    ])
  })

  it("S2, S3: a session archived elsewhere is taken out, and listed again outranks its removal", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    follow()
    gateway.rows.delete("a")
    await advance(timing.pollMs)
    gateway.rows.set("a", row("a"))
    await advance(timing.pollMs)
    expect(updates).toEqual([
      { kind: "session-removed", sessionId: "a", revision: 2 },
      { kind: "session", session: expect.objectContaining({ id: "a", revision: 3 }) },
    ])
  })

  it("S4: reads a watched conversation while it runs, and an idle one only when its row changes", async () => {
    const { gateway, source, follow, advance } = started()
    gateway.rows.set("busy", row("busy", { running: true }))
    gateway.rows.set("quiet", row("quiet"))
    gateway.views.set("busy", view("busy"))
    gateway.views.set("quiet", view("quiet"))
    gateway.views.set("unread", view("unread"))
    gateway.rows.set("unread", row("unread"))
    await source.index()
    await source.transcript("busy")
    await source.transcript("quiet")
    follow()
    const readsOf = (id: string) =>
      gateway.calls.filter((call) => call.method === "read" && call.args[0] === id).length
    await advance(timing.pollMs * 3)
    expect(readsOf("busy")).toBe(4)
    expect(readsOf("quiet")).toBe(1)
    // Never shown, never read.
    expect(readsOf("unread")).toBe(0)
    gateway.rows.set("quiet", row("quiet", { updatedAtMs: 9_000 }))
    await advance(timing.pollMs)
    expect(readsOf("quiet")).toBe(2)
  })

  it("S4: a read that brings a new view is said as the conversation's next revision", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set("a", view("a", { revision: "1" }))
    await source.transcript("a")
    follow()
    gateway.views.set("a", view("a", { revision: "2" }))
    await advance(timing.pollMs)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ sessionId: "a", revision: 2 }),
    })
  })

  it("S5: a failed poll is a gap: the next list that answers says resync, once", async () => {
    const { gateway, source, updates, follow, advance } = started()
    await source.index()
    follow()
    gateway.once("list", () => Promise.reject(new NessaConnectionClosedError(1006, "")))
    await advance(timing.pollMs)
    expect(updates).toEqual([])
    await advance(timing.pollMs)
    expect(updates).toEqual([{ kind: "resync" }])
    await advance(timing.pollMs)
    expect(updates).toEqual([{ kind: "resync" }])
  })

  it("S5: a watched conversation the gateway no longer holds stops being read, and is no gap", async () => {
    const { gateway, source, updates, follow, advance } = started()
    // Live, so it would be read every round (S7) if it stayed watched.
    gateway.views.set("a", view("a", { messages: [running()] }))
    await source.transcript("a")
    gateway.views.delete("a")
    follow()
    await advance(timing.pollMs * 3)
    expect(gateway.calls.filter((call) => call.method === "read")).toHaveLength(2)
    expect(updates).toEqual([])
  })

  it("S6: polling stops when the last listener leaves", async () => {
    const { gateway, source, advance } = started()
    const stop = source.subscribe(() => {})
    await advance(timing.pollMs * 2)
    const lists = gateway.count("list")
    expect(lists).toBe(2)
    stop()
    await advance(timing.pollMs * 5)
    expect(gateway.count("list")).toBe(lists)
  })

  it("a summary says a session waits on the person when its conversation asks", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set(
      "a",
      view("a", { messages: [running()], permissions: [permission()] }),
    )
    await source.index()
    follow()
    await source.transcript("a")
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({ id: "a", status: "needs-you", revision: 2 }),
    })
  })
})

describe("writes", () => {
  it("W1: a first message opens its conversation on the chosen agent and model, then sends it under its id", async () => {
    const { gateway, source } = started()
    await source.send({
      sessionId: "s",
      messageId: "m",
      text: "Hello",
      model: { provider: "openai", modelId: "gpt-5" },
      initiator: "person",
      start: { channelId: "gateway-conversations", title: "Hello" },
    })
    expect(gateway.calls).toEqual([
      {
        method: "create",
        args: [{ conversationId: "s", agent: "codex", model: "gpt-5" }],
      },
      {
        method: "send",
        args: ["s", "Hello", [], [], { executionId: "m", requestId: "m" }],
      },
    ])
  })

  it("W2: a message sent again goes under the same ids, so the gateway takes it once", async () => {
    const { gateway, source } = started()
    const message = {
      sessionId: "s",
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
    expect(sends.map((call) => call.args[4])).toEqual([
      { executionId: "m", requestId: "m" },
      { executionId: "m", requestId: "m" },
    ])
  })

  it("W3: approving once answers the option the gateway says allows, then says the conversation", async () => {
    const { gateway, source, updates, follow } = started()
    const asked = permission()
    gateway.views.set(
      "a",
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript("a")
    follow()
    gateway.once("answer", async (normal) => {
      gateway.views.set("a", view("a", { revision: "2", messages: [running()] }))
      return normal()
    })
    await source.approve("a", approvalId(asked), "once", "person")
    // Resolved once taken; the conversation after it follows as an update (W3c).
    await flush()
    expect(gateway.calls.find((call) => call.method === "answer")?.args).toEqual([
      "a",
      "turn",
      "p1",
      "opt-a",
    ])
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ approval: null, revision: 2 }),
    })
  })

  it("W3: approving always is not supported, and nothing is answered", async () => {
    const { gateway, source } = started()
    await expect(
      source.approve("a", approvalId(permission()), "always", "person"),
    ).rejects.toMatchObject({ reason: "not-supported" })
    expect(gateway.calls).toEqual([])
  })

  it("W4: denying answers the option the gateway says denies; none offered is not supported", async () => {
    const { gateway, source } = started()
    const asked = permission()
    gateway.views.set(
      "a",
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.deny("a", approvalId(asked), "person")
    expect(gateway.calls.find((call) => call.method === "answer")?.args[3]).toBe("opt-b")
    const allowOnly = permission({
      permissionId: "p2",
      options: [{ id: "only", label: "Allow", effect: "allow" }],
    })
    gateway.views.set(
      "a",
      view("a", { revision: "2", messages: [running()], permissions: [allowOnly] }),
    )
    await expect(source.deny("a", approvalId(allowOnly), "person")).rejects.toMatchObject(
      {
        reason: "not-supported",
      },
    )
    expect(gateway.count("answer")).toBe(1)
  })

  it("W5: an approval no longer asked, or an id this source never wrote, is not waiting", async () => {
    const { gateway, source } = started()
    gateway.views.set("a", view("a", { messages: [running()] }))
    await expect(
      source.approve("a", approvalId(permission()), "once", "person"),
    ).rejects.toMatchObject({ reason: "not-waiting" })
    await expect(source.deny("a", "not-an-id", "person")).rejects.toMatchObject({
      reason: "not-waiting",
    })
    expect(gateway.count("answer")).toBe(0)
  })

  it("W6: archiving says the removal, at the summary's next revision, before it resolves", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    follow()
    await source.archive("a", "person")
    expect(updates).toEqual([{ kind: "session-removed", sessionId: "a", revision: 2 }])
    expect((await source.index()).sessions).toEqual([])
  })

  it("W6: a list asked before the archive cannot list the session again after it", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    follow()
    const held = deferred<unknown>()
    gateway.once("list", () => held.promise)
    const before = source.index()
    const archived = source.archive("a", "person")
    await flush()
    held.resolve({ conversations: [row("a", { preview: "late" })], complete: true })
    await before
    await archived
    await source.index()
    expect(updates).toEqual([
      { kind: "session", session: expect.objectContaining({ id: "a", revision: 2 }) },
      { kind: "session-removed", sessionId: "a", revision: 3 },
    ])
  })

  it("W7: marking read asks nothing of the gateway; pinning is not supported", async () => {
    const { gateway, source } = started()
    await source.markRead("a")
    await expect(source.setPinned("a", true, "person")).rejects.toMatchObject({
      reason: "not-supported",
    })
    expect(gateway.calls).toEqual([])
  })
})

describe("refusals are typed (F)", () => {
  const rpc = (code: string) => new NessaRpcError(code, "message text nobody parses")

  it("F1: a conversation the gateway does not hold, or deleted, is an unknown session", async () => {
    const { source } = started()
    await expect(source.transcript("missing")).rejects.toMatchObject({
      reason: "unknown-session",
    })
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

  // A bare NessaRpcError comes only from a list or a read, which the client
  // wraps in nothing: certain, since a read takes no effect.
  it("F3: a read refused for good is not supported", () => {
    for (const code of [
      "agent_not_configured",
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
      rpc("a_code_nobody_taught_this_build"),
      new NessaConnectionClosedError(1006, ""),
      new NessaConversationControlError("c", "r", "e", new Error("lost"), false),
    ])
      expect(refusalOf(error)).toMatchObject({ reason: "unavailable" })
  })

  it("F3: a fault that is no answer is passed on as it is, for the window to log", async () => {
    const { gateway, source } = started()
    const fault = new Error("Invalid conversation response")
    gateway.once("list", () => Promise.reject(fault))
    await expect(source.index()).rejects.toBe(fault)
  })
})

describe("round 1's rows", () => {
  it("R7: a read answered after a newer list is filed against the row it was asked under, so the change is read", async () => {
    const { gateway, source, updates, follow, advance } = started()
    // A running turn whose reply is streaming text: no activity, so only the row says it runs.
    const streaming = (revision: string) =>
      view("a", {
        revision,
        messages: [
          {
            ...running(),
            parts: [{ offset: 0, kind: "text", text: "Work", toolId: "", noticeId: "" }],
          },
        ],
      })
    gateway.rows.set("a", row("a", { running: true, updatedAtMs: 1 }))
    gateway.views.set("a", streaming("1"))
    await source.index()
    await source.transcript("a")
    follow()
    // The round's read is slow; meanwhile the turn ends and an index applies the newer row.
    const slow = deferred<unknown>()
    gateway.once("read", () => slow.promise)
    await advance(timing.pollMs)
    gateway.rows.set("a", row("a", { running: false, updatedAtMs: 2 }))
    await source.index()
    gateway.views.set(
      "a",
      view("a", {
        revision: "3",
        messages: [
          {
            ...running(),
            status: "completed",
            parts: [{ offset: 0, kind: "text", text: "Done", toolId: "", noticeId: "" }],
          },
        ],
      }),
    )
    // The slow read answers with the running snapshot it was asked for.
    slow.resolve(streaming("2"))
    await advance(timing.pollMs * 3)
    const last = updates.filter((update) => update.kind === "transcript").pop()
    expect(last).toMatchObject({ transcript: { revision: 3 } })
  })

  it("C5: an archive that never answers settles, and the lists behind it still go", async () => {
    const { gateway, source, advance } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    gateway.once("archive", () => new Promise(() => {}))
    const archived = source.archive("a", "person").catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await archived).toMatchObject({ reason: "unavailable" })
    const lists = gateway.count("list")
    await expect(source.index()).resolves.toMatchObject({ sessions: [expect.anything()] })
    expect(gateway.count("list")).toBe(lists + 1)
  })

  it("W6b: a second archive of a session is refused as unknown, asks nobody, and says no second removal", async () => {
    const { gateway, source, updates, follow } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    follow()
    const outcomes = await Promise.allSettled([
      source.archive("a", "person"),
      source.archive("a", "agent"),
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
    gateway.views.set("s", view("s", { runtime: runtime(runs.modelId) }))
    await source.transcript("s")
    const asking = (messageId: string, chosen: { provider: string; modelId: string }) =>
      source.send({
        sessionId: "s",
        messageId,
        text: "Again",
        model: { provider: chosen.provider, modelId: chosen.modelId },
        initiator: "person",
      })
    await expect(asking("m1", other)).rejects.toMatchObject({ reason: "not-supported" })
    // Carrying `start` changes nothing: only the gateway's word counts.
    await expect(
      source.send({
        sessionId: "s",
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

  it("W8: a first message and a resend on another model, before any read, record nothing the gateway did not say", async () => {
    const { gateway, source } = started()
    const start = { channelId: "gateway-conversations", title: "Hi" }
    await source.send({
      sessionId: "s",
      messageId: "m1",
      text: "Hi",
      model,
      initiator: "person",
      start,
    })
    const other = { provider: "openai", modelId: "gpt-5" }
    // Not read yet: it cannot be told, so it goes — and it runs on what the gateway created.
    await source.send({
      sessionId: "s",
      messageId: "m2",
      text: "Hi",
      model: other,
      initiator: "person",
      start,
    })
    // Each first message opens (`create` reopens, keeping the model it was created with).
    expect(gateway.count("create")).toBe(2)
    expect(gateway.count("send")).toBe(2)
  })

  it("W8: an unknown model cannot be compared, so the message goes", async () => {
    const { gateway, source } = started()
    gateway.views.set("a", view("a"))
    await source.transcript("a")
    await source.send({
      sessionId: "a",
      messageId: "m",
      text: "Hi",
      model: { provider: "openai", modelId: "gpt-5" },
      initiator: "person",
    })
    expect(gateway.count("send")).toBe(1)
  })

  it("S7: a watched conversation no list names is read while live, and not once at rest", async () => {
    const { gateway, source, follow, advance } = started()
    gateway.views.set("live", view("live", { messages: [running()] }))
    gateway.views.set("rest", view("rest"))
    await source.transcript("live")
    await source.transcript("rest")
    follow()
    await advance(timing.pollMs * 3)
    const readsOf = (id: string) =>
      gateway.calls.filter((call) => call.method === "read" && call.args[0] === id).length
    expect(readsOf("live")).toBe(4)
    expect(readsOf("rest")).toBe(1)
  })

  it("W3b: an answer taken whose read after fails still resolves, and the next list resyncs", async () => {
    const { gateway, source, updates, follow, advance } = started()
    const asked = permission()
    gateway.views.set(
      "a",
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript("a")
    follow()
    gateway.once("read", async (normal) => normal())
    gateway.once("read", () => Promise.reject(new NessaConnectionClosedError(1006, "")))
    await expect(
      source.approve("a", approvalId(asked), "once", "person"),
    ).resolves.toBeUndefined()
    expect(gateway.count("answer")).toBe(1)
    await advance(timing.pollMs)
    expect(updates).toContainEqual({ kind: "resync" })
  })
})

describe("round 1's ownership rows", () => {
  it("S3b: a session listed again after its removal is not said to wait on the person from its old read", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set(
      "a",
      view("a", { messages: [running()], permissions: [permission()] }),
    )
    await source.index()
    await source.transcript("a")
    follow()
    gateway.rows.delete("a")
    await advance(timing.pollMs)
    gateway.rows.set("a", row("a"))
    await advance(timing.pollMs)
    const relisted = updates.filter((update) => update.kind === "session").pop()
    expect(relisted).toMatchObject({ session: { id: "a", status: "idle" } })
  })

  it("a summary is said again when what its conversation runs on comes to be known", async () => {
    const { gateway, source, updates, follow } = started()
    const listed = composerModels[composerModels.length - 1]
    gateway.rows.set("a", row("a"))
    await source.index()
    follow()
    gateway.views.set(
      "a",
      view("a", {
        runtime: {
          model: listed.modelId,
          provider: "x",
          workspace: "/",
          agent: "x",
          modelName: "x",
          contextWindowTokens: 1,
          reasoning: false,
        },
      }),
    )
    await source.transcript("a")
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({
        revision: 2,
        model: { provider: listed.provider, modelId: listed.modelId },
      }),
    })
  })
})

describe("round 2's rows", () => {
  it("F5: a message the client refuses before sending is not supported, not a fault", async () => {
    const { gateway, source } = started()
    gateway.client.conversation.send = () => {
      throw new TypeError("Message must contain 1-8192 UTF-8 bytes")
    }
    await expect(
      source.send({
        sessionId: "a",
        messageId: "m",
        text: "x".repeat(9000),
        model,
        initiator: "person",
      }),
    ).rejects.toMatchObject({ reason: "not-supported" })
  })

  it("S3c: a read answered after its session was taken out is let go, even once it is listed again", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set("a", view("a", { revision: "1", messages: [running()] }))
    await source.index()
    await source.transcript("a")
    follow()
    const held = deferred<unknown>()
    gateway.once("read", () => held.promise)
    await advance(timing.pollMs)
    gateway.rows.delete("a")
    await source.index()
    gateway.rows.set("a", row("a"))
    held.resolve(
      view("a", { revision: "2", messages: [running()], permissions: [permission()] }),
    )
    await source.index()
    await flush()
    const said = updates.filter((update) => update.kind === "session").pop()
    expect(said).toMatchObject({ session: { id: "a", status: "idle" } })
    expect(updates).not.toContainEqual(
      expect.objectContaining({
        kind: "transcript",
        transcript: expect.objectContaining({ revision: 2 }),
      }),
    )
  })

  it("F4: a refusal for good the client cannot say was refused before it ran may have been done", () => {
    const rpc = (code: string) => new NessaRpcError(code, "message text nobody parses")
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
    ).toMatchObject({ reason: "not-supported" })
    // Where the target stands holds whatever the command did.
    expect(
      refusalOf(
        new NessaConversationControlError("c", "r", "e", rpc("stale_permission"), true),
      ),
    ).toMatchObject({ reason: "not-waiting" })
  })
})

describe("the structural change after round 3", () => {
  it("C6: an approval whose call settled unavailable is never sent, so the person's later answer is the one taken", async () => {
    const { gateway, source, advance } = started()
    const asked = permission()
    const asking = view("a", {
      revision: "1",
      messages: [running()],
      permissions: [asked],
    })
    gateway.views.set("a", asking)
    await source.transcript("a")
    // A read ahead of the approval's never answers; the approval's own read
    // starts only when that one times out, and answers after the approval's
    // call has settled.
    gateway.once("read", () => new Promise(() => {}))
    const ownRead = deferred<unknown>()
    gateway.once("read", () => ownRead.promise)
    void source.transcript("a").catch(() => undefined)
    await advance(1_000)
    const approving = source
      .approve("a", approvalId(asked), "once", "person")
      .catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await approving).toMatchObject({ reason: "unavailable" })
    ownRead.resolve(asking)
    await flush()
    expect(gateway.count("answer")).toBe(0)
    // The person answers again, and that is the answer taken.
    await source.deny("a", approvalId(asked), "person")
    expect(
      gateway.calls
        .filter((call) => call.method === "answer")
        .map((call) => call.args[3]),
    ).toEqual(["opt-b"])
  })

  it("C6: an archive whose call settled unavailable is never sent", async () => {
    const { gateway, source, advance } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    // Two lists ahead of the archive: the second starts when the first times
    // out, and ends after the archive's call has settled.
    gateway.once("list", () => new Promise(() => {}))
    const second = deferred<unknown>()
    gateway.once("list", () => second.promise)
    void source.index().catch(() => undefined)
    void source.index().catch(() => undefined)
    await advance(1_000)
    const archiving = source.archive("a", "person").catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await archiving).toMatchObject({ reason: "unavailable" })
    second.resolve({ conversations: [row("a")], complete: true })
    await flush()
    expect(gateway.count("archive")).toBe(0)
  })

  it("W8: the model is checked at the send, after anything the call waited on", async () => {
    const { gateway, source } = started()
    const runs = composerModels[composerModels.length - 1]
    const other = composerModels.find(
      (each) => each.modelId !== runs.modelId || each.provider !== runs.provider,
    )!
    gateway.views.set("s", view("s", { runtime: runtime(runs.modelId) }))
    // While the first message's create is on its way, a read says what the conversation runs.
    gateway.once("create", async (normal) => {
      await source.transcript("s")
      return normal()
    })
    await expect(
      source.send({
        sessionId: "s",
        messageId: "m",
        text: "Hi",
        model: { provider: other.provider, modelId: other.modelId },
        initiator: "person",
        start: { channelId: "gateway-conversations", title: "Hi" },
      }),
    ).rejects.toMatchObject({ reason: "not-supported" })
    expect(gateway.count("send")).toBe(0)
  })

  it("S8: a read let go across a removal is asked again when the session is held again, and the poller keeps following it", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set("a", view("a", { revision: "1", messages: [running()] }))
    await source.index()
    await source.transcript("a")
    follow()
    const held = deferred<unknown>()
    gateway.once("read", () => held.promise)
    await advance(timing.pollMs)
    gateway.rows.delete("a")
    await source.index()
    gateway.rows.set("a", row("a", { running: true }))
    await source.index()
    // The window shows it again; then the stale read answers.
    const shown = source.transcript("a")
    gateway.views.set("a", view("a", { revision: "2", messages: [running()] }))
    held.resolve(view("a", { revision: "old", messages: [running()] }))
    await expect(shown).resolves.toMatchObject({ revision: 2 })
    gateway.views.set("a", view("a", { revision: "3", messages: [running()] }))
    await advance(timing.pollMs * 2)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ sessionId: "a", revision: 3 }),
    })
  })

  it("S8: an approval asked while its session is taken out and listed again is still answered", async () => {
    const { gateway, source, advance } = started()
    const asked = permission()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set(
      "a",
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.index()
    await source.transcript("a")
    const held = deferred<unknown>()
    gateway.once("read", () => held.promise)
    const approving = source.approve("a", approvalId(asked), "once", "person")
    await flush()
    gateway.rows.delete("a")
    await source.index()
    gateway.rows.set("a", row("a", { running: true }))
    await source.index()
    held.resolve(
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await advance(1)
    await expect(approving).resolves.toBeUndefined()
    expect(gateway.count("answer")).toBe(1)
  })

  it("R8: a session taken out is not read, not sent to, and not followed", async () => {
    const { gateway, source, follow, advance } = started()
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a", { messages: [running()] }))
    await source.index()
    await source.archive("a", "person")
    await expect(source.transcript("a")).rejects.toMatchObject({
      reason: "unknown-session",
    })
    await expect(
      source.send({
        sessionId: "a",
        messageId: "m",
        text: "Hi",
        model,
        initiator: "person",
      }),
    ).rejects.toMatchObject({ reason: "unknown-session" })
    follow()
    await advance(timing.pollMs * 3)
    expect(gateway.count("read")).toBe(0)
    expect(gateway.count("send")).toBe(0)
  })

  it("S9: a session taken out during a round is not read later in that round", async () => {
    const { gateway, source, follow, advance } = started()
    for (const id of ["a", "b"]) {
      gateway.rows.set(id, row(id, { running: true }))
      gateway.views.set(id, view(id, { messages: [running()] }))
    }
    await source.index()
    await source.transcript("a")
    await source.transcript("b")
    follow()
    // a's read in the round is slow; meanwhile b is archived.
    const slow = deferred<unknown>()
    gateway.once("read", () => slow.promise)
    await advance(timing.pollMs)
    await source.archive("b", "person")
    const readsOfB = gateway.calls.filter(
      (call) => call.method === "read" && call.args[0] === "b",
    ).length
    slow.resolve(view("a", { messages: [running()] }))
    await flush()
    expect(
      gateway.calls.filter((call) => call.method === "read" && call.args[0] === "b")
        .length,
    ).toBe(readsOfB)
  })

  it("W3c: an answer taken resolves even while the read after it hangs", async () => {
    const { gateway, source } = started()
    const asked = permission()
    gateway.views.set(
      "a",
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript("a")
    gateway.once("read", async (normal) => normal())
    gateway.once("read", () => new Promise(() => {}))
    await expect(
      source.approve("a", approvalId(asked), "once", "person"),
    ).resolves.toBeUndefined()
    expect(gateway.count("answer")).toBe(1)
  })

  it("S8: a read crossed by a removal twice answers unknown-session rather than asking forever", async () => {
    const { gateway, source } = started()
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a"))
    await source.index()
    const flap = async () => {
      gateway.rows.delete("a")
      await source.index()
      gateway.rows.set("a", row("a"))
      await source.index()
    }
    gateway.once("read", async (normal) => {
      await flap()
      return normal()
    })
    gateway.once("read", async (normal) => {
      await flap()
      return normal()
    })
    await expect(source.transcript("a")).rejects.toMatchObject({
      reason: "unknown-session",
    })
    expect(gateway.count("read")).toBe(2)
  })

  it("C6: a first message whose call settled before the gateway answered sends neither create nor message", async () => {
    const gateway = fakeGateway()
    const arriving = deferred<FakeGateway["client"]>()
    const { source, advance } = started(gateway, () => arriving.promise)
    void source.index().catch(() => undefined)
    const sending = source
      .send({
        sessionId: "s",
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
        sessionId: "s",
        messageId: "m",
        text: "Hi",
        model,
        initiator: "person",
        start: { channelId: "gateway-conversations", title: "Hi" },
      })
      .catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await sending).toMatchObject({ reason: "unavailable" })
    created.resolve({ conversationId: "s" })
    await flush()
    expect(gateway.count("send")).toBe(0)
  })
})

describe("through the window's store, unchanged", () => {
  it("opens on the gateway's conversations, shows one, follows it, and resyncs on reconnect", async () => {
    const { gateway, source, advance } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set(
      "a",
      view("a", {
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
    expect(Object.keys(state().sessions)).toEqual(["a"])
    expect(state().transcripts.a?.messages.map(messageText)).toEqual([
      "Run the tests",
      "Working on it",
    ])
    gateway.views.set("a", view("a", { revision: "2", messages: [running("turn")] }))
    await advance(timing.pollMs)
    expect(state().transcripts.a?.revision).toBe(2)
    const lists = gateway.count("list")
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    gateway.setState({ status: "connected" })
    await flush()
    expect(gateway.count("list")).toBe(lists + 1)
  })

  it("sends a message the window's conversation then shows under its id", async () => {
    const { gateway, source, advance } = started()
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a"))
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    await flush()
    gateway.once("send", async (normal) => {
      const { executionId } = gateway.calls[gateway.calls.length - 1].args[4] as {
        executionId: string
      }
      gateway.rows.set("a", row("a", { running: true, updatedAtMs: 3_000 }))
      gateway.views.set(
        "a",
        view("a", {
          revision: "2",
          messages: [{ ...running(executionId), userText: "Hello there" }],
        }),
      )
      return normal()
    })
    const outcome = await store.dispatch(
      sendMessage({ sessionId: "a", text: "Hello there", initiator: "person" }),
    )
    expect(outcome).toBe("sent")
    expect(store.getState().workspace.outbox.a).toHaveLength(1)
    await advance(timing.pollMs)
    // The gateway's conversation holds it under the id it was sent with: the outbox lets it go.
    expect(store.getState().workspace.outbox.a).toBeUndefined()
    expect(store.getState().workspace.transcripts.a?.messages.map(messageText)).toEqual([
      "Hello there",
    ])
  })
})
