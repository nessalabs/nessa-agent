// @vitest-environment jsdom
/**
 * The gateway source against a fake client, one or more tests per row of the
 * table on #248 (row ids in the test names): connection (C), reads (R), the
 * stream (S), writes (W) and refusals (F).
 */
import {
  NessaConnectionClosedError,
  NessaCredentialUnavailableError,
  RetryableConnectError,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  type ConversationPermission,
} from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import { HostRefusalError } from "../../../../host/startup-refusals"
import { SessionHealthError } from "../../../../session/adapters/client/dev-session"
import { WorkspaceSourceError, type WorkspaceUpdate } from "../../application/ports"
import { composerModels } from "../../../model/composer-options"
import { messageText } from "../../model/transcript"
import { testStore } from "../../testing"
import { followWorkspace, loadWorkspace, sendMessage } from "../store/commands"
import { approvalId } from "./gateway-views"
import { deferred, fakeGateway, row, view, type FakeGateway } from "./fake-gateway"
import { gatewaySource, refusalOf, type GatewayClock } from "./gateway-source"

const timing = { callMs: 5_000, pollMs: 100, activePollMs: 100, reconnectRounds: 5 }
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

describe("a connection that could not be made says why (#419)", () => {
  const quiet = () => vi.spyOn(console, "warn").mockImplementation(() => {})
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
    const calls = [source.index(), source.transcript("a"), source.connected()].map(
      (call) => call.catch((error: unknown) => error),
    )
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
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a"))
    const store = testStore(source)
    await store.dispatch(loadWorkspace())
    await flush()
    // Gone for good; the next call connects again, and is refused.
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(4001, ""),
    })
    const outcome = await store.dispatch(
      sendMessage({ sessionId: "a", text: "Hello there", initiator: "person" }),
    )
    expect(outcome).toBe("refused")
    expect(gateway.count("send")).toBe(0)
    expect(store.getState().workspace.outbox.a?.[0]?.delivery).toEqual({
      state: "failed",
      reason: "signed-out",
    })
    warn.mockRestore()
  })

  it("S8: an index that cannot be read says signed out, and Try Again reads it again", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    let refuse = true
    const { source } = started(gateway, () =>
      refuse ? Promise.reject(unauthorized) : Promise.resolve(gateway.client),
    )
    gateway.rows.set("a", row("a"))
    const store = testStore(source)
    await store.dispatch(loadWorkspace())
    expect(store.getState().workspace).toMatchObject({
      status: "failed",
      failure: "signed-out",
    })
    refuse = false
    await store.dispatch(loadWorkspace())
    expect(store.getState().workspace.status).toBe("ready")
    expect(Object.keys(store.getState().workspace.sessions)).toEqual(["a"])
    warn.mockRestore()
  })

  it("S9: an index that failed is a gap: the first poll that answers after it opens the workspace", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    let refuse = true
    const { source, advance } = started(gateway, () =>
      refuse
        ? Promise.reject(new NessaConnectionClosedError(1006, ""))
        : Promise.resolve(gateway.client),
    )
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a"))
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    expect(store.getState().workspace).toMatchObject({
      status: "failed",
      failure: "not-listening",
    })
    // The gateway comes up before the poller's first connect: no poll failed
    // in between. That connect is `reconnectRounds + 1` rounds away (S16).
    refuse = false
    await advance(timing.pollMs * timing.reconnectRounds)
    expect(store.getState().workspace.status).toBe("failed")
    await advance(timing.pollMs)
    await flush()
    expect(store.getState().workspace.status).toBe("ready")
    expect(Object.keys(store.getState().workspace.sessions)).toEqual(["a"])
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
      "a",
      view("a", { messages: [running()], permissions: [permission()] }),
    )
    await source.transcript("a")
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(4001, ""),
    })
    await expect(
      source.approve("a", approvalId(permission()), "once", "person", "opt-a"),
    ).rejects.toMatchObject({ reason: "signed-out" })
    expect(gateway.count("answer")).toBe(0)
    warn.mockRestore()
  })

  it("S7: an archive whose connection is refused is not sent, and says signed out", async () => {
    const warn = quiet()
    const { gateway, source } = signedOutAfterFirst()
    gateway.rows.set("a", row("a"))
    await source.index()
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(4001, ""),
    })
    await expect(source.archive("a", "person")).rejects.toMatchObject({
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

  /** The rounds, from 1, in which the poller connected over `rounds` rounds. */
  async function connectRounds(
    advance: (ms: number) => Promise<void>,
    state: { attempts: number },
    rounds: number,
  ) {
    const at: number[] = []
    for (let round = 1; round <= rounds; round++) {
      const before = state.attempts
      await advance(timing.pollMs)
      if (state.attempts > before) at.push(round)
    }
    return at
  }

  it("S10: after a failed connect the poller connects again only every reconnectRounds + 1 rounds", async () => {
    const warn = quiet()
    const { source, follow, advance, state } = counted()
    follow()
    await source.index().catch(() => undefined)
    expect(state.attempts).toBe(1)
    expect(await connectRounds(advance, state, 18)).toEqual([6, 12, 18])
    warn.mockRestore()
  })

  it("S12: a person's call connects at once while the poller waits, and its failure starts the wait again", async () => {
    const warn = quiet()
    const { source, follow, advance, state } = counted()
    follow()
    await source.index().catch(() => undefined)
    await advance(timing.pollMs * 3)
    await expect(source.index()).rejects.toMatchObject({ reason: "signed-out" })
    expect(state.attempts).toBe(2)
    // Five more rounds from the person's failure, not from the first.
    expect(await connectRounds(advance, state, 6)).toEqual([6])
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
    // No wait is left over: the first round after the close connects.
    expect(await connectRounds(advance, state, 1)).toEqual([1])
    warn.mockRestore()
  })

  it("S14: an MCP App's call while waiting is refused without connecting, and the wait is unchanged", async () => {
    const warn = quiet()
    const { source, follow, advance, state } = counted()
    follow()
    await source.index().catch(() => undefined)
    await advance(timing.pollMs * 2)
    for (let call = 0; call < 3; call++)
      await expect(source.connected()).rejects.toMatchObject({ reason: "unavailable" })
    expect(state.attempts).toBe(1)
    // Still the first failure's wait: round 6 from it, three rounds from here.
    expect(await connectRounds(advance, state, 4)).toEqual([4])
    warn.mockRestore()
  })

  it("S15: a client closed for good mid-round stops the round: no read connects in its place", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    const { source, follow, advance, state } = counted(gateway)
    state.refuse = false
    const ids = ["a", "b", "c", "d", "e"]
    for (const id of ids) {
      gateway.rows.set(id, row(id, { running: true }))
      gateway.views.set(id, view(id, { messages: [running()] }))
    }
    follow()
    await source.index()
    for (const id of ids) await source.transcript(id)
    expect(state.attempts).toBe(1)
    gateway.once("read", async (normal) => {
      state.refuse = true
      gateway.setState({
        status: "closed",
        error: new NessaConnectionClosedError(4001, ""),
      })
      return normal()
    })
    await advance(timing.pollMs)
    // The round that saw the close connected nothing; the next connects once.
    expect(state.attempts).toBe(1)
    await advance(timing.pollMs)
    expect(state.attempts).toBe(2)
    warn.mockRestore()
  })

  it("S15: a poller read queued behind a person's read does not connect once the client has closed", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    const { source, follow, advance, state } = counted(gateway)
    state.refuse = false
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set("a", view("a", { messages: [running()] }))
    follow()
    await source.index()
    await source.transcript("a")
    expect(state.attempts).toBe(1)
    // The person's next read of "a" hangs on the gateway; a poll round lists,
    // and its read of "a" queues behind it.
    const hanging = deferred<unknown>()
    gateway.once("read", () => hanging.promise)
    const personRead = source.transcript("a").catch((error: unknown) => error)
    await flush()
    await advance(timing.pollMs)
    state.refuse = true
    gateway.setState({
      status: "closed",
      error: new NessaConnectionClosedError(4001, ""),
    })
    hanging.reject(new NessaConnectionClosedError(4001, ""))
    await personRead
    await flush()
    // The poller's read ran on no client and connected nothing.
    expect(state.attempts).toBe(1)
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
    // Every round in the budget joined the one attempt; then the wait.
    expect(state.attempts).toBe(1)
    expect(await connectRounds(advance, state, 6)).toEqual([6])
  })

  it("S18: while the source waits, apps and rounds join a person's connect on its way", async () => {
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
    // A person's call connects at once, during the wait; an app and the
    // rounds asking meanwhile join it.
    const person = source.index()
    await flush()
    expect(attempts).toBe(2)
    const app = source.connected()
    await advance(timing.pollMs * 2)
    pending.resolve(gateway.client)
    await expect(app).resolves.toBe(gateway.client)
    await expect(person).resolves.toMatchObject({ sessions: [] })
    expect(attempts).toBe(2)
    warn.mockRestore()
  })

  for (const [path, call] of [
    [
      "opening a session",
      (source: ReturnType<typeof started>["source"]) => source.transcript("a"),
    ],
    [
      "a send",
      (source: ReturnType<typeof started>["source"]) =>
        source.send({
          sessionId: "a",
          messageId: "m",
          text: "hi",
          model,
          initiator: "person",
        }),
    ],
    [
      "an answer",
      (source: ReturnType<typeof started>["source"]) =>
        source.approve("a", approvalId(permission()), "once", "person", "opt-a"),
    ],
    [
      "an archive",
      (source: ReturnType<typeof started>["source"]) => source.archive("a", "person"),
    ],
  ] as const)
    it(`S12: ${path} connects at once while the source waits`, async () => {
      const warn = quiet()
      const gateway = fakeGateway()
      gateway.rows.set("a", row("a"))
      gateway.views.set(
        "a",
        view("a", { messages: [running()], permissions: [permission()] }),
      )
      const { source, state } = counted(gateway)
      await source.index().catch(() => undefined)
      state.refuse = false
      await call(source).catch(() => undefined)
      expect(state.attempts).toBe(2)
      warn.mockRestore()
    })

  it("W3′: the read after an answer does not connect once the client has closed", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    gateway.views.set(
      "a",
      view("a", { messages: [running()], permissions: [permission()] }),
    )
    const { source, state } = counted(gateway)
    state.refuse = false
    await source.transcript("a")
    gateway.once("answer", async (normal) => {
      const answered = await normal()
      gateway.setState({
        status: "closed",
        error: new NessaConnectionClosedError(1006, ""),
      })
      return answered
    })
    await source.approve("a", approvalId(permission()), "once", "person", "opt-a")
    await flush()
    expect(state.attempts).toBe(1)
    warn.mockRestore()
  })

  it("S18′: a poller read joins a connect on its way rather than being refused", async () => {
    const warn = quiet()
    const gateway = fakeGateway()
    let attempts = 0
    const pending = deferred<FakeGateway["client"]>()
    const { source, follow, advance } = started(gateway, () =>
      ++attempts === 1 ? Promise.resolve(gateway.client) : pending.promise,
    )
    for (const id of ["a", "b"]) {
      gateway.rows.set(id, row(id, { running: true }))
      gateway.views.set(id, view(id, { messages: [running()] }))
    }
    follow()
    await source.index()
    await source.transcript("a")
    const readsOfA = () =>
      gateway.calls.filter((call) => call.method === "read" && call.args[0] === "a")
        .length
    const before = readsOfA()
    // The poll's list answers; the client then closes, and a person's
    // connect is on its way when the poller reads "a".
    let person: Promise<unknown> = Promise.resolve()
    gateway.once("list", async (normal) => {
      const listed = await normal()
      gateway.setState({
        status: "closed",
        error: new NessaConnectionClosedError(1006, ""),
      })
      person = source.transcript("b")
      await flush()
      return listed
    })
    await advance(timing.pollMs)
    pending.resolve(gateway.client)
    await person
    await flush()
    expect(attempts).toBe(2)
    expect(readsOfA()).toBe(before + 1)
    warn.mockRestore()
  })

  it("S20: an index that answers after a gap is the resync: the next poll says none", async () => {
    const gateway = fakeGateway()
    const { source, follow, advance, updates } = started(gateway)
    follow()
    await source.index()
    gateway.once("list", () => Promise.reject(rpcCode("temporarily_unavailable")))
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    await source.index()
    updates.length = 0
    await advance(timing.pollMs)
    expect(kinds(updates)).not.toContain("resync")
  })

  it("S9′: an index refused on a held client is a gap the next poll resyncs", async () => {
    const gateway = fakeGateway()
    const { source, follow, advance, updates } = started(gateway)
    follow()
    await source.index()
    gateway.once("list", () => Promise.reject(rpcCode("temporarily_unavailable")))
    await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
    updates.length = 0
    await advance(timing.pollMs)
    expect(kinds(updates)).toContain("resync")
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
    expect(late.count("list")).toBe(0)
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
    // Its not-found takes it out (R9), and is no gap: nothing says resync.
    expect(kinds(updates)).toEqual(["session-removed"])
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
    await source.approve("a", approvalId(asked), "once", "person", "opt-a")
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

  it("W3: approving one of two allow options answers that option", async () => {
    const { gateway, source } = started()
    const asked = permission({
      options: [
        { id: "first", label: "Ship", effect: "allow" },
        { id: "second", label: "Run", effect: "allow" },
      ],
    })
    gateway.views.set(
      "a",
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.transcript("a")
    await source.approve("a", approvalId(asked), "once", "person", "second")
    expect(gateway.calls.find((call) => call.method === "answer")?.args[3]).toBe("second")
  })

  it("W3: approving always is not supported, and nothing is answered", async () => {
    const { gateway, source } = started()
    await expect(
      source.approve("a", approvalId(permission()), "always", "person", "opt-a"),
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
    await source.deny("a", approvalId(asked), "person", "opt-b")
    expect(gateway.calls.find((call) => call.method === "answer")?.args[3]).toBe("opt-b")
    const allowOnly = permission({
      permissionId: "p2",
      options: [{ id: "only", label: "Allow", effect: "allow" }],
    })
    gateway.views.set(
      "a",
      view("a", { revision: "2", messages: [running()], permissions: [allowOnly] }),
    )
    await expect(
      source.deny("a", approvalId(allowOnly), "person", "only"),
    ).rejects.toMatchObject({
      reason: "not-supported",
    })
    expect(gateway.count("answer")).toBe(1)
  })

  it("W5: an approval no longer asked, or an id this source never wrote, is not waiting", async () => {
    const { gateway, source } = started()
    gateway.views.set("a", view("a", { messages: [running()] }))
    await expect(
      source.approve("a", approvalId(permission()), "once", "person", "opt-a"),
    ).rejects.toMatchObject({ reason: "not-waiting" })
    await expect(source.deny("a", "not-an-id", "person", "opt-b")).rejects.toMatchObject({
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

  it("traces a refused conversation.read with the session, the subject, and the gateway code", async () => {
    const debug = vi.spyOn(console, "debug").mockImplementation(() => {})
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    try {
      const { gateway, source } = started()
      gateway.once("read", () => Promise.reject(rpcCode("temporarily_unavailable")))
      await expect(source.transcript("a")).rejects.toMatchObject({
        reason: "unavailable",
      })
      expect(debug).toHaveBeenCalledWith("[nessa] conversation read asked", {
        subject: "conversation",
        method: "conversation.read",
        sessionId: "a",
      })
      expect(warn).toHaveBeenCalledWith("[nessa] conversation read refused", {
        subject: "conversation",
        method: "conversation.read",
        sessionId: "a",
        reason: "unavailable",
        code: "temporarily_unavailable",
      })
      expect(JSON.stringify(warn.mock.calls)).not.toContain("message text nobody parses")

      debug.mockClear()
      warn.mockClear()
      gateway.once("read", () =>
        Promise.reject(new NessaConnectionClosedError(1001, "going away")),
      )
      await expect(source.transcript("a")).rejects.toMatchObject({
        reason: "unavailable",
      })
      expect(warn).toHaveBeenCalledWith("[nessa] conversation read refused", {
        subject: "conversation",
        method: "conversation.read",
        sessionId: "a",
        reason: "unavailable",
        code: "1001",
        closeReason: "transport_interrupted",
        socket: "closed",
      })
      expect(JSON.stringify(warn.mock.calls)).not.toContain("going away")

      debug.mockClear()
      warn.mockClear()
      gateway.once("list", () => Promise.reject(rpcCode("temporarily_unavailable")))
      await expect(source.index()).rejects.toMatchObject({ reason: "unavailable" })
      expect(debug).toHaveBeenCalledWith("[nessa] conversation read asked", {
        subject: "index",
        method: "conversation.list",
      })
      expect(warn).toHaveBeenCalledWith("[nessa] conversation read refused", {
        subject: "index",
        method: "conversation.list",
        reason: "unavailable",
        code: "temporarily_unavailable",
      })
    } finally {
      debug.mockRestore()
      warn.mockRestore()
    }
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
      source.approve("a", approvalId(asked), "once", "person", "opt-a"),
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
      .approve("a", approvalId(asked), "once", "person", "opt-a")
      .catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await approving).toMatchObject({ reason: "unavailable" })
    ownRead.resolve(asking)
    await flush()
    expect(gateway.count("answer")).toBe(0)
    // The person answers again, and that is the answer taken.
    await source.deny("a", approvalId(asked), "person", "opt-b")
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
    // out — its caller, a moment younger, still waiting (R10) — and ends after
    // the archive's call has settled.
    gateway.once("list", () => new Promise(() => {}))
    const second = deferred<unknown>()
    gateway.once("list", () => second.promise)
    void source.index().catch(() => undefined)
    await advance(1)
    void source.index().catch(() => undefined)
    await advance(999)
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
    const approving = source.approve("a", approvalId(asked), "once", "person", "opt-a")
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
      source.approve("a", approvalId(asked), "once", "person", "opt-a"),
    ).resolves.toBeUndefined()
    expect(gateway.count("answer")).toBe(1)
  })

  it("S8: a read crossed by a removal twice, its session held again, answers unavailable rather than asking forever", async () => {
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
      reason: "unavailable",
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

describe("round 5's rows", () => {
  it("R3: a session whose first read fails is followed all the same, and the poller mends it", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set("a", view("a", { messages: [running()] }))
    await source.index()
    gateway.once("read", () => Promise.reject(rpcCode("temporarily_unavailable")))
    await expect(source.transcript("a")).rejects.toMatchObject({ reason: "unavailable" })
    follow()
    await advance(timing.pollMs)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ sessionId: "a", revision: 1 }),
    })
  })

  it("S5: the read after an answer finding the session gone is no gap", async () => {
    const { gateway, source, updates, follow, advance } = started()
    const asked = permission()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set(
      "a",
      view("a", { revision: "1", messages: [running()], permissions: [asked] }),
    )
    await source.index()
    await source.transcript("a")
    follow()
    gateway.once("read", async (normal) => normal())
    const after = deferred<unknown>()
    gateway.once("read", () => after.promise)
    await source.approve("a", approvalId(asked), "once", "person", "opt-a")
    await source.archive("a", "person")
    after.resolve(view("a", { revision: "2", messages: [running()] }))
    await flush()
    // A gap would show as a resync on the next list.
    await advance(timing.pollMs)
    expect(kinds(updates)).not.toContain("resync")
  })

  it("R8, W6b: a session archived before any list named it is remembered as taken out", async () => {
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
    await source.archive("s", "person")
    await expect(source.archive("s", "person")).rejects.toMatchObject({
      reason: "unknown-session",
    })
    await expect(
      source.send({
        sessionId: "s",
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
})

describe("review after ready", () => {
  it("R9: a read the gateway answers as gone takes the session out, even past an incomplete list", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.rows.set("a", row("a", { running: true }))
    gateway.views.set("a", view("a", { messages: [running()] }))
    await source.index()
    await source.transcript("a")
    follow()
    // The list stops naming it, but says it is incomplete; the read says it is gone.
    gateway.complete = false
    gateway.rows.delete("a")
    gateway.views.delete("a")
    await advance(timing.pollMs)
    expect(updates).toContainEqual({
      kind: "session-removed",
      sessionId: "a",
      revision: 2,
    })
    expect((await source.index()).sessions).toEqual([])
    expect(kinds(updates)).not.toContain("resync")
  })

  it("R9: a read answered gone after its session was taken out and listed again is asked again, and takes nothing out", async () => {
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
    const removedBefore = updates.filter((update) => update.kind === "session-removed")
    held.reject(rpcCode("conversation_not_found"))
    await flush()
    expect(updates.filter((update) => update.kind === "session-removed")).toEqual(
      removedBefore,
    )
    expect((await source.index()).sessions).toEqual([
      expect.objectContaining({ id: "a" }),
    ])
    // Asked again for the session listed again, which answers.
    expect(gateway.count("read")).toBe(3)
  })

  it("R10: a read whose caller was answered while it waited its turn asks nothing", async () => {
    const { gateway, source, advance } = started()
    gateway.views.set("a", view("a"))
    // Two reads that never answer: the second begins only when the first
    // times out, so a third, asked meanwhile, outlives its caller in the queue.
    gateway.once("read", () => new Promise(() => {}))
    gateway.once("read", () => new Promise(() => {}))
    const calls = [source.transcript("a")]
    await advance(1)
    calls.push(source.transcript("a"))
    await advance(1)
    calls.push(source.transcript("a"))
    await advance(timing.callMs * 2)
    for (const settled of await Promise.all(
      calls.map((call) => call.catch((error: unknown) => error)),
    ))
      expect(settled).toMatchObject({ reason: "unavailable" })
    expect(gateway.count("read")).toBe(2)
  })

  it("R10: a list whose caller was answered while it waited its turn asks nothing", async () => {
    const { gateway, source, advance } = started()
    gateway.once("list", () => new Promise(() => {}))
    gateway.once("list", () => new Promise(() => {}))
    const calls = [source.index()]
    await advance(1)
    calls.push(source.index())
    await advance(1)
    calls.push(source.index())
    await advance(timing.callMs * 2)
    for (const settled of await Promise.all(
      calls.map((call) => call.catch((error: unknown) => error)),
    ))
      expect(settled).toMatchObject({ reason: "unavailable" })
    expect(gateway.count("list")).toBe(2)
  })

  it("R10: a read sent while its caller waited, answered after the caller was answered, is let go", async () => {
    const { gateway, source, advance } = started()
    gateway.views.set("a", view("a", { revision: "now" }))
    // The first read never answers; the second is sent when it times out,
    // with its caller nearly out of time, and answers after that.
    gateway.once("read", () => new Promise(() => {}))
    const late = deferred<unknown>()
    gateway.once("read", () => late.promise)
    const first = source.transcript("a").catch((error: unknown) => error)
    await advance(timing.callMs - 1)
    const second = source.transcript("a").catch((error: unknown) => error)
    await advance(1)
    expect(gateway.count("read")).toBe(2)
    await advance(timing.callMs - 1)
    expect(await first).toMatchObject({ reason: "unavailable" })
    expect(await second).toMatchObject({ reason: "unavailable" })
    late.resolve(view("a", { revision: "late" }))
    await flush()
    // The late view moved nothing: the next read is the conversation's first count.
    expect((await source.transcript("a")).revision).toBe(1)
  })

  it("R10: a gone read answered after its caller was answered takes nothing out", async () => {
    const { gateway, source, advance } = started()
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a"))
    await source.index()
    gateway.once("read", () => new Promise(() => {}))
    const late = deferred<unknown>()
    gateway.once("read", () => late.promise)
    void source.transcript("a").catch(() => undefined)
    await advance(timing.callMs - 1)
    const second = source.transcript("a").catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await second).toMatchObject({ reason: "unavailable" })
    late.reject(rpcCode("conversation_not_found"))
    await flush()
    // Still held: a session taken out would be refused unknown-session (R8).
    await expect(source.transcript("a")).resolves.toMatchObject({ sessionId: "a" })
  })

  it("R10: a list sent while its caller waited, answered after the caller was answered, is let go", async () => {
    const { gateway, source, advance } = started()
    gateway.once("list", () => new Promise(() => {}))
    const late = deferred<unknown>()
    gateway.once("list", () => late.promise)
    const first = source.index().catch((error: unknown) => error)
    await advance(timing.callMs - 1)
    const second = source.index().catch((error: unknown) => error)
    await advance(1)
    expect(gateway.count("list")).toBe(2)
    await advance(timing.callMs - 1)
    expect(await first).toMatchObject({ reason: "unavailable" })
    expect(await second).toMatchObject({ reason: "unavailable" })
    late.resolve({ conversations: [row("z")], complete: false })
    await flush()
    // An incomplete list removes nothing, so a "z" applied late would still be held.
    gateway.complete = false
    expect((await source.index()).sessions).toEqual([])
  })

  it("R11: a watched conversation refused for good stops being read, is no gap, and Try Again watches it again", async () => {
    const { gateway, source, updates, follow, advance } = started()
    // Live, so it would be read every round (S7) if it stayed watched.
    gateway.views.set("a", view("a", { messages: [running()] }))
    await source.transcript("a")
    follow()
    // Refused by the poller's read, then by the window's Try Again.
    for (let i = 0; i < 2; i++)
      gateway.once("read", () => Promise.reject(rpcCode("conversation_state_unreadable")))
    await advance(timing.pollMs * 3)
    expect(gateway.count("read")).toBe(2)
    expect(kinds(updates)).not.toContain("resync")
    // Try Again: the window reads it once more, and follows it again.
    await expect(source.transcript("a")).rejects.toMatchObject({
      reason: "not-supported",
    })
    await expect(source.transcript("a")).resolves.toMatchObject({ sessionId: "a" })
    const reads = gateway.count("read")
    await advance(timing.pollMs)
    expect(gateway.count("read")).toBeGreaterThan(reads)
  })

  it("R11: a first read refused for good is not followed", async () => {
    const { gateway, source, updates, follow, advance } = started()
    gateway.views.set("a", view("a", { messages: [running()] }))
    follow()
    gateway.once("read", () => Promise.reject(rpcCode("conversation_state_unreadable")))
    await expect(source.transcript("a")).rejects.toMatchObject({
      reason: "not-supported",
    })
    await advance(timing.pollMs * 3)
    expect(gateway.count("read")).toBe(1)
    expect(kinds(updates)).not.toContain("resync")
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
    mcp: { server: "mcptest", tool: "show_chart", resourceUri: "ui://t/chart.html" },
  }

  it("each view applied is told to the apps in the order read, and the same view is not told twice", async () => {
    const { gateway, source, told } = withApps()
    gateway.views.set("a", view("a", { revision: "1" }))
    await source.transcript("a")
    await source.transcript("a")
    gateway.views.set("a", view("a", { revision: "2" }))
    await source.transcript("a")
    expect(told).toEqual([
      ["observe", "a", "1"],
      ["observe", "a", "2"],
    ])
  })

  it("a read let go is not told: one that timed out, or crossed a removal", async () => {
    const { gateway, source, told, advance } = withApps()
    gateway.views.set("a", view("a", { revision: "late" }))
    const held = deferred<unknown>()
    gateway.once("read", () => held.promise)
    const call = source.transcript("a").catch((error: unknown) => error)
    await advance(timing.callMs)
    expect(await call).toMatchObject({ reason: "unavailable" })
    held.resolve(view("a", { revision: "late" }))
    await flush()
    expect(told).toEqual([])
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
    gateway.views.set("a", view("a", { tools, messages }))
    gateway.views.set("b", view("b", { tools, messages }))
    const widgetOf = async (id: string) =>
      (await source.transcript(id)).messages
        .flatMap((message) => message.parts)
        .find((part) => part.kind === "widget")
    const [a, b] = [await widgetOf("a"), await widgetOf("b")]
    expect(a).toBeDefined()
    expect(b).toBeDefined()
    expect(a).not.toEqual(b)
  })

  it("a conversation the gateway says was deleted is forgotten by the apps, after it is taken out", async () => {
    const { gateway, source, told, updates, follow } = withApps()
    gateway.rows.set("a", row("a"))
    await source.index()
    follow()
    gateway.once("read", () => Promise.reject(rpcCode("conversation_deleted")))
    await expect(source.transcript("a")).rejects.toMatchObject({
      reason: "unknown-session",
    })
    expect(kinds(updates)).toContain("session-removed")
    expect(told).toEqual([["forget", "a"]])
  })

  it("a deletion the client reports as a control error is forgotten too", async () => {
    const { gateway, source, told } = withApps()
    gateway.once("read", () =>
      Promise.reject(
        new NessaConversationControlError(
          "a",
          "r",
          undefined,
          rpcCode("conversation_deleted"),
        ),
      ),
    )
    await source.transcript("a").catch(() => undefined)
    expect(told).toEqual([["forget", "a"]])
  })

  it("not found, an archive, or a complete list that leaves it out forgets nothing: a closed or archived conversation comes back", async () => {
    const { gateway, source, told } = withApps()
    // Not found may be this caller's alone.
    await source.transcript("missing").catch(() => undefined)
    // Archived here.
    gateway.rows.set("b", row("b"))
    await source.index()
    await source.archive("b", "person")
    // Missing from a complete list: archived or deleted elsewhere, which a list cannot tell.
    gateway.rows.set("c", row("c"))
    await source.index()
    gateway.rows.delete("c")
    await source.index()
    expect(told).toEqual([])
  })

  it("a deletion answered after its session was taken out and listed again forgets nothing", async () => {
    const { gateway, source, told, follow, advance } = withApps()
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
    held.reject(rpcCode("conversation_deleted"))
    await flush()
    expect(told.filter(([kind]) => kind === "forget")).toEqual([])
  })

  it("the apps' fault is theirs: the read still applies and is said", async () => {
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
    gateway.views.set("a", view("a"))
    await expect(source.transcript("a")).resolves.toMatchObject({ sessionId: "a" })
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

describe("an app's review is read after its turn ended (#436)", () => {
  // The rows of the state table on #436: P1–P9.
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

  /** A conversation whose turn has ended, read once and followed: no list row will move again. */
  async function idle(ids: readonly string[] = ["a"], setup = started()) {
    for (const id of ids) {
      setup.gateway.rows.set(id, row(id))
      setup.gateway.views.set(id, view(id, { revision: "1", messages: [ended] }))
    }
    await setup.source.index()
    for (const id of ids) await setup.source.transcript(id)
    setup.follow()
    const readsOf = (id: string) =>
      setup.gateway.calls.filter((call) => call.method === "read" && call.args[0] === id)
        .length
    return { ...setup, readsOf }
  }

  it("P1: with no app call, an idle conversation whose row is unchanged is not read again", async () => {
    const { readsOf, advance } = await idle()
    await advance(timing.pollMs * 3)
    expect(readsOf("a")).toBe(1)
  })

  it("P2, P3: while an app's call is unanswered its conversation is read each round, and the review it opens is shown as the app's", async () => {
    const { gateway, source, updates, readsOf, advance } = await idle()
    const answer = deferred<string>()
    const call = source.appCall("a", () => answer.promise)
    await advance(timing.pollMs)
    // P2: read, though the row did not move.
    expect(readsOf("a")).toBe(2)
    // P3: the gateway opens the app's review; the next round reads it.
    gateway.views.set(
      "a",
      view("a", { revision: "2", messages: [ended], permissions: [appReview] }),
    )
    await advance(timing.pollMs)
    expect(readsOf("a")).toBe(3)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({
        sessionId: "a",
        approval: {
          id: approvalId(appReview),
          command: "app_delete_row {}",
          reason: "An app asks to run app_delete_row on mcptest",
          origin: { kind: "app", server: "mcptest", tool: "app_delete_row" },
          options: [
            { id: "allow", label: "Allow", choice: "once" },
            { id: "deny", label: "Deny", choice: "deny" },
          ],
        },
      }),
    })
    expect(updates).toContainEqual({
      kind: "session",
      session: expect.objectContaining({ id: "a", status: "needs-you" }),
    })
    answer.resolve("answered")
    // The call settles as the app's own call did.
    expect(await call).toBe("answered")
  })

  it("P4: the person's answer to an app's review goes to the gateway by the option's id, then the conversation is read", async () => {
    const { gateway, source, updates, advance } = await idle()
    const answer = deferred<string>()
    void source.appCall("a", () => answer.promise)
    gateway.views.set(
      "a",
      view("a", { revision: "2", messages: [ended], permissions: [appReview] }),
    )
    await advance(timing.pollMs)
    gateway.once("answer", async (normal) => {
      gateway.views.set("a", view("a", { revision: "3", messages: [ended] }))
      answer.resolve("allowed")
      return normal()
    })
    await source.approve("a", approvalId(appReview), "once", "person", "allow")
    await flush()
    expect(gateway.calls.find((call) => call.method === "answer")?.args).toEqual([
      "a",
      "turn",
      "app-1",
      "allow",
    ])
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ approval: null, revision: 3 }),
    })
  })

  it("P5: a call that settles elsewhere is read until a read shows its review gone, then no more", async () => {
    const { gateway, source, updates, readsOf, advance } = await idle()
    const answer = deferred<string>()
    const call = source.appCall("a", () => answer.promise)
    gateway.views.set(
      "a",
      view("a", { revision: "2", messages: [ended], permissions: [appReview] }),
    )
    await advance(timing.pollMs)
    // It expired on the gateway: the call is answered, and the review is gone from the view.
    gateway.views.set("a", view("a", { revision: "3", messages: [ended] }))
    answer.reject(new Error("expired"))
    await expect(call).rejects.toThrow("expired")
    const before = readsOf("a")
    await advance(timing.pollMs)
    // The last read still showed the review, so this round reads, and finds it gone.
    expect(readsOf("a")).toBe(before + 1)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ approval: null, revision: 3 }),
    })
    await advance(timing.pollMs * 3)
    expect(readsOf("a")).toBe(before + 1)
  })

  it("P6: with two calls unanswered, one settling leaves the conversation read until the other does", async () => {
    const { source, readsOf, advance } = await idle()
    const first = deferred<string>()
    const second = deferred<string>()
    const one = source.appCall("a", () => first.promise)
    const two = source.appCall("a", () => second.promise)
    first.resolve("one")
    await one
    await advance(timing.pollMs * 2)
    expect(readsOf("a")).toBe(3)
    second.resolve("two")
    await two
    const before = readsOf("a")
    await advance(timing.pollMs * 2)
    expect(readsOf("a")).toBe(before)
  })

  it("P7: a call that is refused, or throws before it is sent, stops the reads as an answer does, and is passed on as it was", async () => {
    const { source, readsOf, advance } = await idle()
    const refusal = new Error("mcp_tool_not_for_app")
    await expect(source.appCall("a", () => Promise.reject(refusal))).rejects.toBe(refusal)
    const thrown = new TypeError("past the bounds")
    await expect(
      source.appCall("a", () => {
        throw thrown
      }),
    ).rejects.toBe(thrown)
    await advance(timing.pollMs * 3)
    expect(readsOf("a")).toBe(1)
  })

  it("P8: a call in one conversation reads that conversation alone", async () => {
    const { source, readsOf, advance } = await idle(["a", "b"])
    const answer = deferred<string>()
    void source.appCall("a", () => answer.promise)
    await advance(timing.pollMs * 2)
    expect(readsOf("a")).toBe(3)
    expect(readsOf("b")).toBe(1)
    answer.resolve("done")
  })

  it("P9: a conversation taken out is not read for an app's call, which still goes to the gateway", async () => {
    const { gateway, source, readsOf, advance } = await idle()
    gateway.rows.delete("a")
    await source.index()
    const asked = vi.fn(() => Promise.resolve("answered"))
    await expect(source.appCall("a", asked)).resolves.toBe("answered")
    const hanging = deferred<string>()
    void source.appCall("a", () => hanging.promise)
    await advance(timing.pollMs * 3)
    expect(asked).toHaveBeenCalledTimes(1)
    expect(readsOf("a")).toBe(1)
    hanging.resolve("done")
  })

  it.each(["conversation_state_unreadable", "conversation_deleted"])(
    "P9: a conversation the gateway will not show again (%s) is not read for an app's call, before or after it",
    async (code) => {
      const { gateway, source, readsOf, advance } = await idle()
      const first = deferred<string>()
      const one = source.appCall("a", () => first.promise)
      gateway.once("read", async () => {
        throw rpcCode(code)
      })
      await advance(timing.pollMs)
      const refused = readsOf("a")
      expect(refused).toBe(2)
      await advance(timing.pollMs * 3)
      expect(readsOf("a")).toBe(refused)
      first.resolve("one")
      await one
      // A later call does not bring it back into the rounds.
      const second = deferred<string>()
      void source.appCall("a", () => second.promise)
      await advance(timing.pollMs * 3)
      expect(readsOf("a")).toBe(refused)
      second.resolve("two")
    },
  )

  it("P2: a conversation no list has named yet is read each round while an app's call waits, and not at rest", async () => {
    const { gateway, source, follow, advance } = started()
    // Past an incomplete list that does not name it (S7).
    gateway.complete = false
    gateway.views.set("a", view("a", { revision: "1", messages: [ended] }))
    await source.index()
    await source.transcript("a")
    follow()
    const readsOf = () =>
      gateway.calls.filter((call) => call.method === "read" && call.args[0] === "a")
        .length
    await advance(timing.pollMs * 2)
    expect(readsOf()).toBe(1)
    const answer = deferred<string>()
    const call = source.appCall("a", () => answer.promise)
    await advance(timing.pollMs * 2)
    expect(readsOf()).toBe(3)
    answer.resolve("done")
    await call
    await advance(timing.pollMs * 2)
    expect(readsOf()).toBe(3)
  })

  it("P2: a call begun while a round's list is still on its way is read in that round", async () => {
    const { gateway, source, readsOf, advance } = await idle()
    const list = deferred<void>()
    gateway.once("list", async (normal) => {
      await list.promise
      return normal()
    })
    await advance(timing.pollMs)
    const answer = deferred<string>()
    void source.appCall("a", () => answer.promise)
    list.resolve()
    await flush()
    expect(readsOf("a")).toBe(2)
    answer.resolve("done")
  })

  it("P2 (reconnect): a call that spans a reconnect keeps its conversation read each round, and not at rest once it settles", async () => {
    const { gateway, source, updates, readsOf, advance } = await idle()
    const answer = deferred<string>()
    const call = source.appCall("a", () => answer.promise)
    await advance(timing.pollMs)
    expect(readsOf("a")).toBe(2)
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    gateway.setState({ status: "connected" })
    expect(updates).toContainEqual({ kind: "resync" })
    // The reconnect does not end the call: it is still read each round.
    await advance(timing.pollMs * 2)
    expect(readsOf("a")).toBe(4)
    answer.resolve("done")
    await call
    // The last read showed no review: at rest again (P1).
    await advance(timing.pollMs * 3)
    expect(readsOf("a")).toBe(4)
  })

  it("P2 (new client): a call that spans a close for good keeps its conversation read each round on the new client, and not at rest once it settles", async () => {
    const gateway = fakeGateway()
    const next = fakeGateway()
    next.rows.set("a", row("a"))
    next.views.set("a", view("a", { revision: "1", messages: [ended] }))
    let attempts = 0
    const { source, updates, advance } = await idle(
      ["a"],
      started(gateway, () =>
        Promise.resolve(++attempts === 1 ? gateway.client : next.client),
      ),
    )
    const readsOnNext = () =>
      next.calls.filter((call) => call.method === "read" && call.args[0] === "a").length
    const answer = deferred<string>()
    const call = source.appCall("a", () => answer.promise)
    await advance(timing.pollMs)
    gateway.setState({ status: "closed", error: new Error("gone") })
    // Making the new client takes more than one round's microtasks (C3).
    await source.index()
    expect(attempts).toBe(2)
    expect(updates).toContainEqual({ kind: "resync" })
    // The new client does not end the call: it is still read each round.
    await advance(timing.pollMs * 2)
    expect(readsOnNext()).toBe(2)
    answer.resolve("done")
    await call
    // The last read showed no review: at rest again (P1).
    await advance(timing.pollMs * 3)
    expect(readsOnNext()).toBe(2)
  })

  it("P2 (read lost): a round's read that the reconnect makes fail does not drop the call; its conversation is read on the next rounds", async () => {
    const { gateway, source, readsOf, advance } = await idle()
    const answer = deferred<string>()
    const call = source.appCall("a", () => answer.promise)
    const held = deferred<never>()
    gateway.once("read", () => held.promise)
    await advance(timing.pollMs)
    expect(readsOf("a")).toBe(2)
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    held.reject(new NessaConnectionClosedError(1006, ""))
    gateway.setState({ status: "connected" })
    await advance(timing.pollMs * 2)
    expect(readsOf("a")).toBe(4)
    answer.resolve("done")
    await call
    await advance(timing.pollMs * 3)
    expect(readsOf("a")).toBe(4)
  })

  it("P2 (settles while reconnecting): a call that settles while the client reconnects is read once more, which shows the review gone, then at rest", async () => {
    const { gateway, source, updates, readsOf, advance } = await idle()
    const answer = deferred<string>()
    const call = source.appCall("a", () => answer.promise)
    gateway.views.set(
      "a",
      view("a", { revision: "2", messages: [ended], permissions: [appReview] }),
    )
    await advance(timing.pollMs)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ approval: expect.anything() }),
    })
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, ""),
    })
    gateway.views.set("a", view("a", { revision: "3", messages: [ended] }))
    answer.resolve("done")
    await call
    gateway.setState({ status: "connected" })
    const before = readsOf("a")
    await advance(timing.pollMs)
    expect(readsOf("a")).toBe(before + 1)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ approval: null, revision: 3 }),
    })
    await advance(timing.pollMs * 3)
    expect(readsOf("a")).toBe(before + 1)
  })
})

describe("independent active transcript polling (#532)", () => {
  function fast() {
    const gateway = fakeGateway()
    const { clock, advance } = manualClock()
    const source = gatewaySource({
      connect: () => Promise.resolve(gateway.client),
      clock,
      timing: { ...timing, pollMs: 1_000, activePollMs: 250 },
    })
    const updates: WorkspaceUpdate[] = []
    const follow = () => source.subscribe((update) => updates.push(update))
    const busy = async (id: string) => {
      gateway.rows.set(id, row(id, { running: true }))
      gateway.views.set(id, view(id, { messages: [running()] }))
      await source.transcript(id)
    }
    return { gateway, source, advance, updates, follow, busy }
  }
  it("F1: reads active text at 250 ms without another list", async () => {
    const { gateway, source, advance, updates, follow, busy } = fast()
    await busy("a")
    await source.index()
    follow()
    updates.length = 0
    gateway.views.set("a", view("a", { revision: "r2", messages: [running()] }))
    await advance(249)
    expect(updates).toEqual([])
    await advance(1)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ revision: 2 }),
    })
    expect(gateway.count("list")).toBe(1)
    source.dispose()
  })
  it("F2/F7: a held list and read do not block another conversation or queue bursts", async () => {
    const { gateway, source, advance, updates, follow, busy } = fast()
    await busy("slow")
    await busy("fast")
    await source.index()
    follow()
    const list = deferred<unknown>(),
      slow = deferred<unknown>()
    gateway.once("list", () => list.promise)
    gateway.once("read", () => slow.promise)
    await advance(1_000)
    updates.length = 0
    gateway.views.set("fast", view("fast", { revision: "r2", messages: [running()] }))
    await advance(250)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ sessionId: "fast", revision: 2 }),
    })
    const slowReads = () =>
      gateway.calls.filter((c) => c.method === "read" && c.args[0] === "slow").length
    expect(slowReads()).toBe(2)
    slow.resolve(view("slow", { revision: "r2", messages: [running()] }))
    await flush()
    expect(slowReads()).toBe(2)
    await advance(250)
    expect(slowReads()).toBe(3)
    source.dispose()
    list.resolve({ conversations: [], complete: false })
    await flush()
  })
  it("F3: resubscription discards an earlier answer", async () => {
    const { gateway, source, advance, updates, follow, busy } = fast()
    await busy("a")
    const stop = follow(),
      pending = deferred<unknown>()
    gateway.once("read", () => pending.promise)
    await advance(250)
    stop()
    follow()
    updates.length = 0
    pending.resolve(view("a", { revision: "old" }))
    await flush()
    expect(updates).toEqual([])
    gateway.views.set("a", view("a", { revision: "fresh", messages: [running()] }))
    await advance(250)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ revision: 2 }),
    })
    source.dispose()
  })
  it("F4: active failure resyncs on the next successful summary", async () => {
    const { gateway, source, advance, updates, follow, busy } = fast()
    await busy("a")
    await source.index()
    follow()
    updates.length = 0
    gateway.once("read", () => Promise.reject(rpcCode("temporarily_unavailable")))
    await advance(250)
    expect(kinds(updates)).not.toContain("resync")
    await advance(750)
    expect(kinds(updates).filter((k) => k === "resync")).toHaveLength(1)
    source.dispose()
  })
  it("F5: send invalidates idle text; settled transcripts stop reading", async () => {
    const { gateway, source, advance, follow } = fast()
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a"))
    await source.index()
    await source.transcript("a")
    follow()
    await source.send({
      sessionId: "a",
      messageId: "m",
      text: "hello",
      model,
      initiator: "person",
    })
    gateway.views.set("a", view("a", { revision: "r2", messages: [running()] }))
    await advance(250)
    expect(gateway.count("read")).toBe(2)
    gateway.views.set("a", view("a", { revision: "r3" }))
    await advance(250)
    const reads = gateway.count("read")
    await advance(2_000)
    expect(gateway.count("read")).toBe(reads)
    source.dispose()
  })
  it("F6: dispose fences a pending answer and both timers", async () => {
    const { gateway, source, advance, updates, follow, busy } = fast()
    await busy("a")
    follow()
    const pending = deferred<unknown>()
    gateway.once("read", () => pending.promise)
    await advance(250)
    source.dispose()
    const reads = gateway.count("read"),
      said = updates.length
    pending.resolve(view("a", { revision: "late" }))
    await advance(2_000)
    expect(gateway.count("read")).toBe(reads)
    expect(updates).toHaveLength(said)
  })
})

describe("subscription and invalidation orderings (#532)", () => {
  it("F8: an idle send remains invalidated across a lost subscription answer", async () => {
    const { gateway, source, follow, advance, updates } = started()
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a"))
    await source.index()
    await source.transcript("a")
    const stop = follow()
    await source.send({
      sessionId: "a",
      messageId: "m",
      text: "hello",
      model,
      initiator: "person",
    })
    const pending = deferred<unknown>()
    gateway.once("read", () => pending.promise)
    await advance(100)
    stop()
    follow()
    updates.length = 0
    pending.resolve(view("a", { revision: "old" }))
    await flush()
    expect(updates).toEqual([])
    gateway.views.set("a", view("a", { revision: "new" }))
    await advance(100)
    expect(updates).toContainEqual({
      kind: "transcript",
      transcript: expect.objectContaining({ revision: 2 }),
    })
    source.dispose()
  })
  it("F8 (failure): a failed invalidated idle read retries without a list change", async () => {
    const gateway = fakeGateway(),
      { clock, advance } = manualClock()
    const source = gatewaySource({
      connect: () => Promise.resolve(gateway.client),
      clock,
      timing: { ...timing, pollMs: 1_000, activePollMs: 250 },
    })
    gateway.rows.set("a", row("a"))
    gateway.views.set("a", view("a"))
    await source.index()
    await source.transcript("a")
    source.subscribe(() => {})
    await source.send({
      sessionId: "a",
      messageId: "m",
      text: "hello",
      model,
      initiator: "person",
    })
    gateway.once("read", () => Promise.reject(rpcCode("temporarily_unavailable")))
    await advance(250)
    expect(gateway.count("read")).toBe(2)
    await advance(250)
    expect(gateway.count("read")).toBe(3)
    expect(gateway.count("list")).toBe(1)
    source.dispose()
  })
  it("F9: an earlier subscription's list cannot remove the current session", async () => {
    const { gateway, source, follow, advance, updates } = started()
    gateway.rows.set("a", row("a"))
    await source.index()
    const stop = follow(),
      pending = deferred<unknown>()
    gateway.once("list", () => pending.promise)
    await advance(100)
    stop()
    follow()
    updates.length = 0
    pending.resolve({ conversations: [], complete: true })
    await flush()
    expect(updates).toEqual([])
    expect((await source.index()).sessions.map((s) => s.id)).toEqual(["a"])
    source.dispose()
  })
})

it("M1: records reproducible polling wait and request cost (#532)", async () => {
  const gateway = fakeGateway(),
    { clock, advance } = manualClock()
  const source = gatewaySource({
    connect: () => Promise.resolve(gateway.client),
    clock,
    timing: { ...timing, pollMs: 1_000, activePollMs: 250 },
  })
  gateway.rows.set("a", row("a", { running: true }))
  gateway.views.set("a", view("a", { messages: [running()] }))
  await source.transcript("a")
  await source.index()
  const delivery = { received: false }
  source.subscribe((update) => {
    if (update.kind === "transcript") delivery.received = true
  })
  const samples: number[] = []
  // Each phase is measured from a whole-second polling boundary in both versions.
  for (let i = 0; i < 20; i++) {
    await advance(2_000 - (clock.now() % 1_000))
    await advance(1 + ((i * 47) % 249))
    gateway.views.set("a", view("a", { revision: `sample-${i}`, messages: [running()] }))
    delivery.received = false
    const start = clock.now()
    for (let ms = 0; ms < 1_100 && !delivery.received; ms++) await advance(1)
    samples.push(clock.now() - start)
  }
  const before = { reads: gateway.count("read"), lists: gateway.count("list") }
  await advance(10_000)
  const cost = {
    reads: gateway.count("read") - before.reads,
    lists: gateway.count("list") - before.lists,
  }
  console.log(
    "532_MEASUREMENT",
    JSON.stringify({ samplesMs: samples, tenSecondCost: cost }),
  )
  source.dispose()
  expect(Math.max(...samples)).toBeLessThanOrEqual(250)
  expect(cost).toEqual({ reads: 40, lists: 10 })
})

describe("foreground activation (#532)", () => {
  for (const entry of ["index", "transcript"] as const) {
    it(`F10 (${entry}): applying active state arms polling from an idle cache`, async () => {
      const gateway = fakeGateway(),
        { clock, advance } = manualClock()
      const source = gatewaySource({
        connect: () => Promise.resolve(gateway.client),
        clock,
        timing: { ...timing, pollMs: 1_000, activePollMs: 250 },
      })
      gateway.rows.set("a", row("a"))
      gateway.views.set("a", view("a"))
      await source.index()
      await source.transcript("a")
      source.subscribe(() => {})
      gateway.views.set("a", view("a", { revision: "running", messages: [running()] }))
      if (entry === "index") {
        gateway.rows.set("a", row("a", { running: true }))
        await source.index()
      } else await source.transcript("a")
      const before = gateway.count("read")
      await advance(250)
      expect(gateway.count("read")).toBe(before + 1)
      source.dispose()
    })
  }
})

it("F11: summary and active timers share minimum background read spacing (#532)", async () => {
  const gateway = fakeGateway(),
    { clock, advance } = manualClock()
  const source = gatewaySource({
    connect: () => Promise.resolve(gateway.client),
    clock,
    timing: { ...timing, pollMs: 1_000, activePollMs: 250 },
  })
  gateway.rows.set("a", row("a", { running: true }))
  gateway.views.set("a", view("a", { messages: [running()] }))
  await source.index()
  await source.transcript("a")
  source.subscribe(() => {})
  const pending = deferred<unknown>()
  gateway.once("list", () => pending.promise)
  await advance(1_000)
  expect(gateway.count("read")).toBe(5)
  await advance(125)
  pending.resolve({ conversations: [...gateway.rows.values()], complete: true })
  await flush()
  expect(gateway.count("read")).toBe(5)
  await advance(125)
  expect(gateway.count("read")).toBe(6)
  source.dispose()
})

it("F12: a background read queued behind foreground work paces from dispatch (#532)", async () => {
  const gateway = fakeGateway(),
    { clock, advance } = manualClock()
  const starts: number[] = []
  const client = {
    ...gateway.client,
    conversation: {
      ...gateway.client.conversation,
      read: (id: string) => {
        starts.push(clock.now())
        return gateway.client.conversation.read(id)
      },
    },
  }
  const source = gatewaySource({
    connect: () => Promise.resolve(client),
    clock,
    timing: { ...timing, pollMs: 1_000, activePollMs: 250 },
  })
  gateway.rows.set("a", row("a", { running: true }))
  gateway.views.set("a", view("a", { messages: [running()] }))
  await source.index()
  await source.transcript("a")
  source.subscribe(() => {})
  const pending = deferred<unknown>()
  gateway.once("read", () => pending.promise)
  const foreground = source.transcript("a")
  await advance(900)
  expect(gateway.count("read")).toBe(2)
  pending.resolve(view("a", { messages: [running()] }))
  await foreground
  await flush()
  expect(gateway.count("read")).toBe(3)
  await advance(100)
  expect(gateway.count("read")).toBe(3)
  await advance(250)
  expect(gateway.count("read")).toBe(4)
  expect(starts.at(-1)! - starts.at(-2)!).toBeGreaterThanOrEqual(250)
  source.dispose()
})
