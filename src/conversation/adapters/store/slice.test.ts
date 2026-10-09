import { describe, expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { scenarioEffects } from "../scenario/effects"
import { textContent } from "../../model"
import type { ConversationView } from "../../application/view"
import type { ConversationFollower } from "../../application/ports"
import {
  renameConversation,
  closeConversation,
  openConversation,
  sendDraft,
  setActive,
  setDraft,
  setSelection,
  stopGenerating,
  controlConversation,
  followConversation,
  unfollowConversation,
  bindConversation,
  restoreConversations,
} from "./slice"
import {
  conversationTabSnapshot,
  parseConversationTabSnapshot,
} from "../../application/saved-tabs"
import { followByReading } from "../../testing"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
const view = (id: string, text = ""): ConversationView => ({
  conversationId: id,
  approvalMode: "ask",
  approvalModes: [
    { id: "ask", name: "Provider asks", description: "Provider asks where required." },
  ],
  title: null,
  revision: text,
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
  truncated: false,
  queueComplete: true,
  transcriptState: "complete",
})
const serverId = (index: number) =>
  `00000000-0000-4000-8000-${index.toString().padStart(12, "0")}`

describe("gateway conversation projection", () => {
  it("restores bounded unique server references and preserves the active conversation", () => {
    const store = makeStore(createDependencies({ conversation: scenarioEffects("echo") }))
    const first = serverId(1)
    const second = serverId(2)
    const saved = parseConversationTabSnapshot({
      tabs: [
        { conversationId: first, title: " First " },
        { conversationId: first, title: "duplicate" },
        { conversationId: second },
        { conversationId: "" },
        ...Array.from({ length: 70 }, (_, index) => ({
          conversationId: serverId(index + 3),
        })),
      ],
      activeConversationId: second,
    })
    if (!saved) throw new Error("valid saved conversation fixture was rejected")
    store.dispatch(restoreConversations(saved))
    const tabs = store.getState().conversation
    expect(tabs.conversations).toHaveLength(64)
    expect(tabs.conversations[0]).toMatchObject({
      title: "First",
      titleEdited: true,
      serverConversationId: first,
      serverReady: true,
      turns: [],
      draft: [],
    })
    expect(tabs.conversations.find((item) => item.id === tabs.activeId)).toMatchObject({
      serverConversationId: second,
    })
    const snapshot = conversationTabSnapshot(tabs)
    expect(snapshot.activeConversationId).toBe(second)
    expect(snapshot.tabs.slice(0, 2)).toEqual([
      { conversationId: first, title: "First" },
      { conversationId: second },
    ])
  })

  it("rejects storage without valid server references", () => {
    expect(
      parseConversationTabSnapshot({
        tabs: [{ conversationId: "" }, { conversationId: "x".repeat(257) }],
      }),
    ).toBeNull()
    expect(parseConversationTabSnapshot(null)).toBeNull()
    expect(parseConversationTabSnapshot([])).toBeNull()
  })

  /**
   * The contract the composer's full-pane editor rests on: a draft this store
   * turned away rejects *with a reason*, and one it took does not. Only a
   * failure after the message was admitted throws without one, because by then
   * the draft has gone and the transcript owns it.
   *
   * Without this, a composer can only know that it called `submit`, which is
   * the same call whether the draft left or is still sitting in the editor.
   */
  it("says whether a draft was taken, so a caller need not guess", async () => {
    const store = makeStore(createDependencies({ conversation: scenarioEffects("echo") }))

    const taken = await store.dispatch(sendDraft({ content: textContent("hello") }))
    expect(sendDraft.rejected.match(taken)).toBe(false)

    // Over the 8 KiB this gateway accepts. The draft is kept and the person is
    // told; nothing left the composer.
    const tooLarge = await store.dispatch(
      sendDraft({ content: textContent("x".repeat(8193)) }),
    )
    expect(sendDraft.rejected.match(tooLarge)).toBe(true)
    expect(tooLarge.payload).toEqual({ kind: "message-too-large" })

    // And a draft with nothing in it is refused the same way, rather than
    // fulfilling as though it had been sent.
    const empty = await store.dispatch(sendDraft({ content: textContent("   ") }))
    expect(sendDraft.rejected.match(empty)).toBe(true)
    expect(empty.payload).toEqual({ kind: "empty-draft" })

    // Including the one that is only reachable by racing a close against a
    // submit: a conversation that is not there took nothing.
    const gone = await store.dispatch(
      sendDraft({ content: textContent("hello"), id: "not-a-conversation" }),
    )
    expect(sendDraft.rejected.match(gone)).toBe(true)
    expect(gone.payload).toEqual({ kind: "no-such-conversation" })
  })

  it("sends through injected effects, retaining server and submission identities", async () => {
    const effects = scenarioEffects("echo")
    const send = vi.fn(effects.send)
    const store = makeStore(createDependencies({ conversation: { ...effects, send } }))
    await store.dispatch(sendDraft({ content: textContent("hello") }))
    const conversation = store.getState().conversation.conversations[0]!
    expect(conversation.serverConversationId).toBeTruthy()
    expect(send).toHaveBeenCalledWith(
      expect.objectContaining({
        conversationId: conversation.serverConversationId,
        text: "hello",
      }),
    )
    expect(conversation.turns[0]).toMatchObject({
      from: "user",
      receipt: "delivered",
      content: textContent("hello"),
    })
    expect(conversation.turns[1]).toMatchObject({ from: "assistant", text: "hello" })
    expect(conversation.phase).toBe("idle")
  })

  it("queues two sends while the first admission is pending and preserves later drafts", async () => {
    const effects = scenarioEffects("echo")
    const gate = deferred<void>()
    const send = vi.fn(async (input: Parameters<typeof effects.send>[0]) => {
      await gate.promise
      return effects.send(input)
    })
    const store = makeStore(createDependencies({ conversation: { ...effects, send } }))
    const first = store.dispatch(sendDraft({ content: textContent("one"), id: "c0" }))
    const second = store.dispatch(sendDraft({ content: textContent("two"), id: "c0" }))
    store.dispatch(setDraft({ draft: textContent("later"), id: "c0" }))
    await Promise.resolve()
    expect(send).toHaveBeenCalledTimes(2)
    expect(new Set(send.mock.calls.map(([input]) => input.executionId)).size).toBe(2)
    gate.resolve()
    await Promise.all([first, second])
    expect(store.getState().conversation.conversations[0]!.draft).toEqual(
      textContent("later"),
    )
    expect(
      store
        .getState()
        .conversation.conversations[0]!.turns.filter((turn) => turn.from === "user"),
    ).toHaveLength(2)
  })

  it("late acknowledgement stays on its original tab", async () => {
    const effects = scenarioEffects("echo")
    const gate = deferred<void>()
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          send: async (input) => {
            await gate.promise
            return effects.send(input)
          },
        },
      }),
    )
    const pending = store.dispatch(sendDraft({ content: textContent("original") }))
    store.dispatch(openConversation())
    gate.resolve()
    await pending
    expect(store.getState().conversation.activeId).toBe("c1")
    expect(store.getState().conversation.conversations[0]!.turns).toHaveLength(2)
    expect(store.getState().conversation.conversations[1]!.turns).toEqual([])
  })

  it("keeps unknown admission separate from assistant output and retries identical IDs", async () => {
    const effects = scenarioEffects("echo")
    let fail = true
    const send = vi.fn(async (input: Parameters<typeof effects.send>[0]) => {
      if (fail) throw new Error("connection lost")
      return effects.send(input)
    })
    const store = makeStore(createDependencies({ conversation: { ...effects, send } }))
    const content = [
      { type: "pasted-text" as const, id: "paste", text: "  code\n\n<tag>\t\n" },
    ]
    await store.dispatch(sendDraft({ content }))
    const turn = store.getState().conversation.conversations[0]!.turns[0]!
    expect(turn).toMatchObject({ from: "user", receipt: "unknown", content })
    expect(store.getState().conversation.conversations[0]!.turns).toHaveLength(1)
    fail = false
    if (turn.from !== "user" || !turn.executionId)
      throw new Error("missing receipt identity")
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "retry", executionId: turn.executionId },
      }),
    )
    expect(send.mock.calls[1]).toEqual(send.mock.calls[0])
    expect(store.getState().conversation.conversations[0]!.turns[0]).toMatchObject({
      receipt: "delivered",
      content,
    })
  })

  it("refreshes an uncertain replay boundary after observing a newer published input", async () => {
    const effects = scenarioEffects("echo")
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          send: async () => {
            throw new Error("connection lost")
          },
          follow: followByReading(async (id) => ({
            ...view(id, "B"),
            messages: [
              {
                executionId: "B",
                userText: "Later",
                attachments: [],
                files: [],
                parts: [],
                status: "failed",
                authenticationRequired: true,
              },
            ],
          })),
        },
      }),
    )
    await store.dispatch(sendDraft({ content: textContent("local") }))
    const turn = store.getState().conversation.conversations[0]?.turns[0]
    if (!turn || turn.from !== "user" || !turn.executionId)
      throw new Error("missing uncertain submission")
    expect(turn.observedInput).toBeNull()
    await store.dispatch(followConversation("c0"))
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "retry", executionId: turn.executionId },
      }),
    )
    expect(
      store
        .getState()
        .conversation.conversations[0]?.turns.find(
          (held) => held.from === "user" && held.executionId === turn.executionId,
        ),
    ).toMatchObject({ observedInput: "B", receipt: "unknown" })
  })

  it("Stop names the captured turn and leaves the attachment open", async () => {
    const effects = scenarioEffects("echo")
    const gate = deferred<void>()
    const sendGate = deferred<void>()
    const stop = vi.fn(async (id: string, executionId: string) => {
      await gate.promise
      return effects.stop(id, executionId)
    })
    const close = vi.fn(async (id: string) => effects.close(id))
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          stop,
          close,
          send: async (input) => {
            await sendGate.promise
            return effects.send(input)
          },
        },
      }),
    )
    const sending = store.dispatch(sendDraft({ content: textContent("hello") }))
    await Promise.resolve()
    const turn = store.getState().conversation.conversations[0]!.turns[0]!
    if (turn.from !== "user" || !turn.executionId) throw new Error("missing turn")
    const stopping = store.dispatch(stopGenerating(undefined))
    await Promise.resolve()
    expect(stop).toHaveBeenCalledWith(
      store.getState().conversation.conversations[0]!.serverConversationId,
      turn.executionId,
    )
    expect(close).not.toHaveBeenCalled()
    expect(store.getState().conversation.conversations[0]!.controlPending).toBe(true)
    gate.resolve()
    await stopping
    expect(store.getState().conversation.conversations[0]!.controlPending).toBe(false)
    store.dispatch(closeConversation("c0"))
    expect(close).not.toHaveBeenCalled()
    expect(store.getState().conversation.conversations).toHaveLength(1)
    sendGate.resolve()
    await sending
    expect(stop).toHaveBeenCalledTimes(2)
    expect(stop).toHaveBeenNthCalledWith(2, stop.mock.calls[0]![0], turn.executionId)
  })

  it("Stop names the running turn when a later turn is only queued", async () => {
    const effects = scenarioEffects("echo")
    const stop = vi.fn(async () => {})
    const close = vi.fn(async () => {})
    const seen = view("rev")
    seen.conversationId = serverId(1)
    seen.messages = [
      {
        executionId: "active",
        parts: [
          {
            offset: 0,
            kind: "text",
            text: "working",
            toolId: "",
            noticeId: "",
          },
        ],
        userText: "go",
        attachments: [],
        files: [],
        status: "running",
      },
    ]
    seen.pending = [
      {
        executionId: "queued",
        text: "later",
        attachments: [],
        files: [],
        mode: "queued",
      },
    ]
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          stop,
          close,
          follow: followByReading(async () => seen),
        },
      }),
    )
    const id = store.getState().conversation.activeId
    store.dispatch(bindConversation({ id, serverId: serverId(1) }))
    await store.dispatch(followConversation(id))
    await store.dispatch(stopGenerating({ conversationId: id }))
    expect(stop).toHaveBeenCalledWith(serverId(1), "active")
    expect(close).not.toHaveBeenCalled()
  })

  it("discards older reads and detached-tab responses", async () => {
    const effects = scenarioEffects("echo")
    const first = deferred<ConversationView>()
    let calls = 0
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          follow: followByReading(async (id) =>
            ++calls === 1 ? first.promise : view(id, "new"),
          ),
        },
      }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
    const oldRead = store.dispatch(followConversation("c0"))
    await store.dispatch(followConversation("c0"))
    first.resolve(view("server", "old"))
    await oldRead
    expect(store.getState().conversation.conversations[0]!.revision).toBe("new")
    store.dispatch(closeConversation("c0"))
    await store.dispatch(followConversation("c0"))
    expect(store.getState().conversation.conversations[0]!.id).toBe("c1")
  })

  it("P6: keeps a follow the tab's through its views and failures, preserving projection references for an unchanged view", async () => {
    const effects = scenarioEffects("echo")
    let follower: ConversationFollower | undefined
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          follow: (_id, told) => {
            follower = told
            return () => {
              follower = undefined
            }
          },
        },
      }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
    const following = store.dispatch(followConversation("c0"))
    follower!.view(view("server", "same"))
    await following
    const first = store.getState().conversation.conversations[0]!
    expect(first.readRequest).toBe(following.requestId)
    follower!.view(view("server", "same"))
    const unchanged = store.getState().conversation.conversations[0]!
    expect(unchanged.turns).toBe(first.turns)
    expect(unchanged.remote).toBe(first.remote)
    expect(unchanged.readRequest).toBe(following.requestId)
    follower!.failed("unavailable")
    const failed = store.getState().conversation.conversations[0]!
    // The follow goes on: its next view applies and clears the word.
    expect(failed.readRequest).toBe(following.requestId)
    expect(failed.readError).toBe("unavailable")
    follower!.view(view("server", "next"))
    const recovered = store.getState().conversation.conversations[0]!
    expect(recovered.revision).toBe("next")
    expect(recovered.readError).toBeUndefined()
  })

  it("names a plain read failure in the panel's own word", async () => {
    const effects = scenarioEffects("echo")
    const read = vi.fn().mockRejectedValueOnce(new Error("offline"))
    const store = makeStore(
      createDependencies({ conversation: { ...effects, follow: followByReading(read) } }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
    await store.dispatch(followConversation("c0"))
    // The panel's own word for a read it could not make sense of, not the
    // error's text: the scenario substitute rejects with a plain `Error`.
    expect(store.getState().conversation.conversations[0]!.readError).toBe("unavailable")
  })

  it("P4, P7: drops a replaced follow's views, and stops the follow when the tab is unfollowed", async () => {
    const effects = scenarioEffects("echo")
    const followers: ConversationFollower[] = []
    const stops: Array<ReturnType<typeof vi.fn>> = []
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          follow: (_id, told) => {
            followers.push(told)
            const stop = vi.fn()
            stops.push(stop)
            return stop
          },
        },
      }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
    void store.dispatch(followConversation("c0"))
    void store.dispatch(followConversation("c0"))
    // Following again stops the follow it replaces.
    expect(stops[0]).toHaveBeenCalledTimes(1)
    followers[1]!.view(view("server", "new"))
    followers[0]!.view(view("server", "old"))
    expect(store.getState().conversation.conversations[0]!.revision).toBe("new")
    await store.dispatch(unfollowConversation("c0"))
    expect(stops[1]).toHaveBeenCalledTimes(1)
    followers[1]!.view(view("server", "later"))
    expect(store.getState().conversation.conversations[0]!.revision).toBe("new")
  })

  it("P5: after a command, follows a followed tab again and reads an unfollowed one once", async () => {
    const effects = scenarioEffects("echo")
    const stops: Array<ReturnType<typeof vi.fn>> = []
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          follow: (id, told) => {
            const stop = vi.fn()
            stops.push(stop)
            queueMicrotask(() => told.view(view(id, `r${stops.length}`)))
            return stop
          },
        },
      }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
    // Not on screen: the command's result is read once, and nothing is left following.
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "setApprovalMode", mode: "ask" },
      }),
    )
    expect(stops).toHaveLength(1)
    expect(stops[0]).toHaveBeenCalledTimes(1)
    // On screen: followed again, and the new follow goes on.
    await store.dispatch(followConversation("c0"))
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "setApprovalMode", mode: "ask" },
      }),
    )
    expect(stops).toHaveLength(3)
    expect(stops[1]).toHaveBeenCalledTimes(1)
    expect(stops[2]).not.toHaveBeenCalled()
    await store.dispatch(unfollowConversation("c0"))
    expect(stops[2]).toHaveBeenCalledTimes(1)
  })

  it("applies a revised capability snapshot without requiring message changes", async () => {
    const effects = scenarioEffects("echo")
    const initial = view("server", "capabilities:1")
    const revised = view("server", "capabilities:2")
    revised.capabilities.agentFeatures.permissionDenial =
      "supported_for_offered_permission_reviews"
    const read = vi.fn().mockResolvedValueOnce(initial).mockResolvedValueOnce(revised)
    const store = makeStore(
      createDependencies({ conversation: { ...effects, follow: followByReading(read) } }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
    await store.dispatch(followConversation("c0"))
    await store.dispatch(followConversation("c0"))
    const current = store.getState().conversation.conversations[0]!
    expect(current.revision).toBe("capabilities:2")
    expect(current.remote?.capabilities.agentFeatures.permissionDenial).toBe(
      "supported_for_offered_permission_reviews",
    )
  })

  it("routes permission decisions and queue removal with exact targets", async () => {
    const effects = scenarioEffects("echo")
    const answer = vi.fn(async () => {})
    const cancel = vi.fn(async () => {})
    const remove = vi.fn(async () => {})
    const store = makeStore(
      createDependencies({ conversation: { ...effects, answer, cancel, remove } }),
    )
    await store.dispatch(sendDraft({ content: textContent("hello") }))
    const id = store.getState().conversation.conversations[0]!.serverConversationId
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: {
          kind: "answer",
          executionId: "execution",
          permissionId: "permission",
          optionId: "deny",
        },
      }),
    )
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "cancel", executionId: "execution", permissionId: "permission" },
      }),
    )
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "remove", executionId: "queued" },
      }),
    )
    expect(answer).toHaveBeenCalledWith(id, "execution", "permission", "deny")
    expect(cancel).toHaveBeenCalledWith(id, "execution", "permission")
    expect(remove).toHaveBeenCalledWith(id, "queued")
  })

  it("keeps each local draft attached to its tab", () => {
    const store = makeStore()
    store.dispatch(setDraft({ draft: textContent("one") }))
    store.dispatch(openConversation())
    store.dispatch(setDraft({ draft: textContent("two") }))
    store.dispatch(setActive("c0"))
    expect(store.getState().conversation.conversations[0]!.draft).toEqual(
      textContent("one"),
    )
    expect(store.getState().conversation.conversations[1]!.draft).toEqual(
      textContent("two"),
    )
  })
})

it("retains a custom title across the first send and later replacement views", async () => {
  const store = makeStore(createDependencies({ conversation: scenarioEffects("echo") }))
  store.dispatch(renameConversation({ id: "c0", title: " My chat " }))
  await store.dispatch(sendDraft({ content: textContent("First message") }))
  expect(store.getState().conversation.conversations[0]?.title).toBe("My chat")
})

it("freezes a draft's catalog selection at its first gateway identity", async () => {
  const effects = scenarioEffects("echo")
  const create = vi.fn((id: string, selection?: Parameters<typeof effects.create>[1]) =>
    effects.create(id, selection),
  )
  const store = makeStore(createDependencies({ conversation: { ...effects, create } }))
  const selected = { agent: "codex", model: "astra", approvalMode: "auto" } as const
  store.dispatch(setSelection({ id: "c0", selection: selected }))
  await store.dispatch(sendDraft({ content: textContent("Hello") }))
  expect(create.mock.calls[0]?.[1]).toEqual(selected)
  store.dispatch(
    setSelection({
      id: "c0",
      selection: { agent: "claude", model: "sonnet", approvalMode: "ask" },
    }),
  )
  expect(store.getState().conversation.conversations[0]?.selection).toEqual(selected)
})

it("shows cancelling until close acknowledgement and does not claim cancellation after failure", async () => {
  for (const succeeds of [true, false]) {
    const gate = deferred<void>()
    const effects = scenarioEffects("echo")
    const store = makeStore(
      createDependencies({
        conversation: {
          ...effects,
          close: async () => {
            await gate.promise
            if (!succeeds) throw new Error("Close failed")
          },
          follow: followByReading(async () => view("server")),
        },
      }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
    const stopped = store.dispatch(
      controlConversation({ id: "c0", control: { kind: "close" } }),
    )
    expect(store.getState().conversation.conversations[0]?.cancellationStatus).toBe(
      "cancelling",
    )
    gate.resolve()
    await stopped
    expect(store.getState().conversation.conversations[0]?.cancellationStatus).toBe(
      succeeds ? "cancelled" : undefined,
    )
  }
})
