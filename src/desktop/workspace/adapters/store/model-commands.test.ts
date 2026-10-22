import { WorkspaceModelError } from "../../application/ports"
/**
 * Public model command ordering: a pick in an established chat is staged,
 * and the next send asks the source for it before the message goes, with
 * independent source frames and receipts.
 */
import { describe, expect, it, vi } from "vitest"
import { fakeSource, settle, summary, testStore, opus, astra } from "../../testing"
import { emptyTranscript } from "../../model/transcript"
import {
  changeModel,
  archiveSession,
  approve,
  stopTurn,
  followWorkspace,
  loadWorkspace,
  newSession,
  sendMessage,
  resendMessage,
  setComposerText,
} from "./commands"
import { workspaceActions } from "./slice"
import { modelChanging, modelRequesting, modelUnsettled } from "./model-commands"
import { selectNextModel } from "./selectors"
import type { ModelReceipt } from "../../model/model-choice"
import type { ModelRef } from "../../model/workspace-index"

const next = { provider: opus.provider, modelId: "claude-sonnet-5" }
const receipt = (requestId: string): ModelReceipt => ({
  requestId,
  model: next.modelId,
  approvalMode: "ask",
  effect: "nextTurn",
  sourceReconciled: false,
})
async function ready() {
  const source = fakeSource()
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  store.dispatch(followWorkspace())
  await settle()
  return { source, store }
}
/** A control the source makes, saying the chat runs it before it answers. */
let revision = 1000
const reconciles =
  (source: ReturnType<typeof fakeSource>) =>
  async (sessionId: string, model: ModelRef, id: string): Promise<ModelReceipt> => {
    source.emit({
      kind: "session",
      session: summary(sessionId, "desktop", 1000, "idle", {
        model,
        revision: ++revision,
      }),
    })
    return { ...receipt(id), model: model.modelId, sourceReconciled: true }
  }
const asked = (source: ReturnType<typeof fakeSource>, method: string) =>
  source.calls.filter((call) => call[0] === method)
const send = (
  store: Awaited<ReturnType<typeof ready>>["store"],
  text: string,
  sessionId = "a",
) => store.dispatch(sendMessage({ sessionId, text, initiator: "person" }))

describe("model choices", () => {
  it("keeps draft choices local and only stages a pick in an established chat", async () => {
    const { source, store } = await ready()
    const draft = store.dispatch(newSession())!
    store.dispatch(changeModel({ sessionId: draft, model: astra }))
    expect(selectNextModel(store.getState(), draft)).toEqual(astra)
    // Another provider's model: shown in the picker, asked of nobody.
    store.dispatch(changeModel({ sessionId: "a", model: astra }))
    expect(selectNextModel(store.getState(), "a")).toEqual(astra)
    expect(store.getState().workspace.sessions.a?.model).toEqual(opus)
    expect(asked(source, "setModel")).toHaveLength(0)
    expect(modelChanging(store.getState().workspace, "a")).toBe(false)
  })

  it("takes a staged pick back when the chat's own model is picked again", async () => {
    const { source, store } = await ready()
    store.dispatch(changeModel({ sessionId: "a", model: astra }))
    store.dispatch(changeModel({ sessionId: "a", model: { ...opus } }))
    expect(store.getState().workspace.stagedModels.a).toBeUndefined()
    expect(selectNextModel(store.getState(), "a")).toEqual(opus)
    expect(await send(store, "on the same agent")).toBe("sent")
    expect(asked(source, "setModel")).toHaveLength(0)
    expect(asked(source, "send")[0]?.[1]).toMatchObject({ model: opus })
  })

  it("applies the staged pick when the next message is sent, then sends on it", async () => {
    const { source, store } = await ready()
    store.dispatch(changeModel({ sessionId: "a", model: astra }))
    expect(await send(store, "switch now")).toBe("sent")
    expect(source.calls.map((call) => call[0]).filter((m) => m !== "subscribe")).toEqual(
      expect.arrayContaining(["setModel", "send"]),
    )
    const order = source.calls.map((call) => call[0])
    expect(order.indexOf("setModel")).toBeLessThan(order.indexOf("send"))
    expect(asked(source, "setModel")[0]).toEqual([
      "setModel",
      "a",
      astra,
      expect.any(String),
      "person",
    ])
    expect(asked(source, "send")[0]?.[1]).toMatchObject({
      model: astra,
      text: "switch now",
    })
    // The send that made it settles the pick.
    await settle()
    expect(store.getState().workspace.stagedModels.a).toBeUndefined()
    expect(selectNextModel(store.getState(), "a")).toEqual(astra)
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
  })

  it("no longer stages a pick once made, so a later change elsewhere is not undone", async () => {
    const { source, store } = await ready()
    source.setModel = vi.fn(reconciles(source))
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(await send(store, "first on next")).toBe("sent")
    expect(store.getState().workspace.stagedModels.a).toBeUndefined()
    // Another client moves the chat on; the next send asks nothing back.
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 1000, "idle", { model: astra, revision: 5000 }),
    })
    await settle()
    expect(selectNextModel(store.getState(), "a")).toEqual(astra)
    expect(await send(store, "second")).toBe("sent")
    expect(source.setModel).toHaveBeenCalledTimes(1)
    expect(asked(source, "send").at(-1)?.[1]).toMatchObject({ model: astra })
  })

  it("marks the message not sent when the source refuses the change, and drops the pick", async () => {
    const { source, store } = await ready()
    source.refuse("setModel", "not-supported")
    store.dispatch(changeModel({ sessionId: "a", model: astra }))
    expect(await send(store, "held back")).toBe("refused")
    expect(asked(source, "send")).toHaveLength(0)
    const failed = store.getState().workspace.outbox.a![0]!
    expect(failed.delivery).toMatchObject({ state: "failed", reason: "not-supported" })
    expect(store.getState().workspace.modelChoices.a?.state).toBe("refused")
    // The picker shows the model the chat still runs; sent again, it goes on it.
    expect(store.getState().workspace.stagedModels.a).toBeUndefined()
    expect(selectNextModel(store.getState(), "a")).toEqual(opus)
    expect(
      await store.dispatch(
        resendMessage({ sessionId: "a", messageId: failed.id, initiator: "person" }),
      ),
    ).toBe("sent")
    expect(asked(source, "setModel")).toHaveLength(1)
    expect(asked(source, "send")[0]?.[1]).toMatchObject({
      model: opus,
      text: "held back",
    })
  })

  it("keeps a pick not known to be made, and checks it on the next send", async () => {
    const { source, store } = await ready()
    source.setModel = async () => {
      throw new WorkspaceModelError("unavailable", true)
    }
    store.dispatch(changeModel({ sessionId: "a", model: astra }))
    expect(await send(store, "unknown")).toBe("unknown")
    expect(store.getState().workspace.stagedModels.a).toEqual(astra)
    source.setModel = vi.fn(reconciles(source))
    expect(await send(store, "checked")).toBe("sent")
    expect(source.setModel).toHaveBeenCalledWith("a", astra, expect.any(String), "person")
    expect(store.getState().workspace.stagedModels.a).toBeUndefined()
    expect(asked(source, "send").at(-1)?.[1]).toMatchObject({
      model: astra,
      text: "checked",
    })
  })

  it("clears what an earlier pick came to once another is made", async () => {
    const { source, store } = await ready()
    source.refuse("setModel", "not-supported")
    store.dispatch(changeModel({ sessionId: "a", model: astra }))
    await send(store, "refused switch")
    expect(store.getState().workspace.modelChoices.a?.state).toBe("refused")
    store.dispatch(changeModel({ sessionId: "a", model: opus }))
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
    expect(store.getState().workspace.stagedModels.a).toBeUndefined()
  })

  it("keeps a send behind a change on its way, and the text with it", async () => {
    const { source, store } = await ready()
    source.hold("setModel")
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    const first = send(store, "goes first")
    await settle()
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    expect(modelRequesting(store.getState().workspace, "a")).toBe(true)
    store.dispatch(setComposerText({ sessionId: "a", text: "unsent draft" }))
    expect(await send(store, "unsent draft")).toBe("not-asked")
    expect(store.getState().workspace.composerText.a).toBe("unsent draft")
    expect(asked(source, "send")).toHaveLength(0)
    await source.release("setModel")
    expect(await first).toBe("sent")
    expect(asked(source, "setModel")).toHaveLength(1)
  })

  it("does not send on an acknowledged change no frame confirmed, and asks again on the next send", async () => {
    const { source, store } = await ready()
    let calls = 0
    source.setModel = async (_id, model, id) => {
      calls++
      if (calls === 2)
        source.emit({
          kind: "session",
          session: summary("a", "desktop", 1000, "idle", { model, revision: 2 }),
        })
      return { ...receipt(id), model: model.modelId, sourceReconciled: calls === 2 }
    }
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(await send(store, "first try")).toBe("unknown")
    expect(store.getState().workspace.modelChoices.a?.state).toBe("acknowledged")
    expect(modelUnsettled(store.getState().workspace, "a")).toBe(true)
    expect(modelChanging(store.getState().workspace, "a")).toBe(false)
    expect(asked(source, "send")).toHaveLength(0)
    expect(await send(store, "second try")).toBe("sent")
    expect(calls).toBe(2)
    expect(asked(source, "send")[0]?.[1]).toMatchObject({
      model: next,
      text: "second try",
    })
  })

  it("rejects a mismatched receipt without sending", async () => {
    const { source, store } = await ready()
    source.setModel = async () => receipt("wrong")
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(await send(store, "mismatch")).toBe("unknown")
    expect(store.getState().workspace.modelChoices.a?.state).toBe("recovery_required")
    expect(asked(source, "send")).toHaveLength(0)
  })

  it("asks for the shown model before a send while an uncertain change is unresolved", async () => {
    const { source, store } = await ready()
    source.setModel = async () => {
      throw new WorkspaceModelError("unavailable", true)
    }
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(await send(store, "uncertain")).toBe("unknown")
    // Taken back to the chat's own model: still checked before anything goes.
    store.dispatch(changeModel({ sessionId: "a", model: opus }))
    expect(store.getState().workspace.modelChoices.a?.state).toBe("recovery_required")
    source.setModel = vi.fn(reconciles(source))
    expect(modelUnsettled(store.getState().workspace, "a")).toBe(true)
    expect(await send(store, "checked first")).toBe("sent")
    expect(source.setModel).toHaveBeenCalledWith("a", opus, expect.any(String), "person")
    expect(asked(source, "send").at(-1)?.[1]).toMatchObject({
      model: opus,
      text: "checked first",
    })
  })

  it("does not send when the source reconciles to another client's model", async () => {
    const { source, store } = await ready()
    source.setModel = async (_id, _model, id) => {
      source.emit({
        kind: "session",
        session: summary("a", "desktop", 1000, "idle", { model: astra, revision: 3 }),
      })
      return { ...receipt(id), sourceReconciled: true }
    }
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(await send(store, "raced")).toBe("unknown")
    expect(asked(source, "send")).toHaveLength(0)
    expect(store.getState().workspace.stagedModels.a).toEqual(next)
    expect(modelChanging(store.getState().workspace, "a")).toBe(false)
  })

  it("keeps a pick when another client passes through it, and applies it on send", async () => {
    const { source, store } = await ready()
    source.setModel = vi.fn(reconciles(source))
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    for (const [model, revision] of [
      [next, 200],
      [astra, 201],
    ] as const) {
      source.emit({
        kind: "session",
        session: summary("a", "desktop", 1000, "idle", { model, revision }),
      })
      await settle()
    }
    expect(store.getState().workspace.stagedModels.a).toEqual(next)
    expect(await send(store, "on my pick")).toBe("sent")
    expect(source.setModel).toHaveBeenCalledWith("a", next, expect.any(String), "person")
    expect(asked(source, "send").at(-1)?.[1]).toMatchObject({ model: next })
  })

  it("holds a send with a staged pick while this window's access change is on its way", async () => {
    const { source, store } = await ready()
    store.dispatch(changeModel({ sessionId: "a", model: astra }))
    store.dispatch(workspaceActions.accessChanging({ sessionId: "a", mode: "auto" }))
    store.dispatch(setComposerText({ sessionId: "a", text: "waits" }))
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    expect(await send(store, "waits")).toBe("not-asked")
    expect(store.getState().workspace.composerText.a).toBe("waits")
    expect(store.getState().workspace.outbox.a ?? []).toEqual([])
    expect(asked(source, "setModel")).toHaveLength(0)
  })

  it("holds a send while the source says a model change is pending", async () => {
    const { source, store } = await ready()
    source.emit({
      kind: "transcript",
      transcript: {
        ...emptyTranscript("a"),
        revision: 2,
        modelControl: {
          pending: { requestId: "x", requestedModel: next.modelId, status: "changing" },
        },
      },
    })
    await settle()
    store.dispatch(changeModel({ sessionId: "a", model: astra }))
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    expect(await send(store, "waits")).toBe("not-asked")
    expect(asked(source, "setModel")).toHaveLength(0)
    expect(asked(source, "send")).toHaveLength(0)
  })

  it("ignores a stale token or wrong target completion", async () => {
    const { store } = await ready()
    const choice = { requestId: "current", model: next, state: "asking" as const }
    store.dispatch(workspaceActions.modelChoiceAsked({ sessionId: "a", choice }))
    store.dispatch(
      workspaceActions.modelChoiceSettled({
        sessionId: "a",
        choice: { ...choice, requestId: "old", state: "acknowledged" },
      }),
    )
    store.dispatch(
      workspaceActions.modelChoiceSettled({
        sessionId: "b",
        choice: { ...choice, state: "acknowledged" },
      }),
    )
    store.dispatch(
      workspaceActions.modelChoiceCleared({ sessionId: "a", requestId: "old" }),
    )
    expect(store.getState().workspace.modelChoices.a).toEqual(choice)
    expect(store.getState().workspace.modelChoices.b).toBeUndefined()
  })

  it("forgets a removed target's pick and ignores its late receipt", async () => {
    const { source, store } = await ready()
    source.hold("setModel")
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    const pending = send(store, "before archive")
    await settle()
    await store.dispatch(archiveSession({ sessionId: "a", initiator: "person" }))
    await settle()
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
    expect(store.getState().workspace.stagedModels.a).toBeUndefined()
    await source.release("setModel")
    await pending
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
  })

  it("keeps original review answers and Stop independent of a staged pick", async () => {
    const { source, store } = await ready()
    const approval = {
      id: "original",
      command: "cargo test",
      reason: "Tests",
      origin: { kind: "agent" as const },
      ask: "tool" as const,
      options: [{ id: "allow", label: "Allow", choice: "once" as const }],
    }
    source.emit({
      kind: "transcript",
      transcript: {
        ...emptyTranscript("a"),
        revision: 2,
        approvals: [approval],
        activity: { label: "Working", since: 1 },
      },
    })
    await settle()
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(store.getState().workspace.transcripts.a?.approvals).toEqual([approval])
    await store.dispatch(
      approve({
        sessionId: "a",
        approvalId: "original",
        scope: "once",
        initiator: "person",
        optionId: "allow",
      }),
    )
    await store.dispatch(stopTurn({ sessionId: "a", initiator: "person" }))
    expect(asked(source, "approve")).toHaveLength(1)
    expect(asked(source, "stop")).toHaveLength(1)
    expect(asked(source, "setModel")).toHaveLength(0)
  })

  it("clears an acknowledged choice once the source's frame confirms it", async () => {
    const { source, store } = await ready()
    source.setModel = async (_id, _model, id) => receipt(id)
    store.dispatch(changeModel({ sessionId: "a", model: next }))
    await send(store, "acknowledged only")
    expect(store.getState().workspace.modelChoices.a?.state).toBe("acknowledged")
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 1000, "idle", { model: next, revision: 2 }),
    })
    await settle()
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
    expect(modelUnsettled(store.getState().workspace, "a")).toBe(false)
    // The chat runs the pick now: the next send makes it without asking again.
    const asks = asked(source, "setModel").length
    expect(await send(store, "confirmed")).toBe("sent")
    expect(asked(source, "setModel")).toHaveLength(asks)
    expect(store.getState().workspace.stagedModels.a).toBeUndefined()
  })
})
