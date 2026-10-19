import { WorkspaceModelError } from "../../application/ports"
/** Public model command ordering, with independent source frames and receipts. */
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
import { modelChanging, modelRequesting } from "./model-commands"
import { selectNextModel } from "./selectors"
import type { ModelReceipt } from "../../model/model-choice"

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

describe("same-provider model choices", () => {
  it("keeps draft choices local and refuses established cross-provider targets", async () => {
    const { source, store } = await ready()
    const draft = store.dispatch(newSession())!
    await store.dispatch(changeModel({ sessionId: draft, model: astra }))
    expect(selectNextModel(store.getState(), draft)).toEqual(astra)
    await store.dispatch(changeModel({ sessionId: "a", model: astra }))
    expect(source.calls.some((call) => call[0] === "setModel")).toBe(false)
    expect(selectNextModel(store.getState(), "a")).toEqual(opus)
  })

  it("keeps the confirmed model until an ACK then independent matching frame", async () => {
    const { source, store } = await ready()
    let requestId = ""
    source.setModel = vi.fn(async (_id, _model, id) => {
      requestId = id
      return receipt(id)
    })
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(selectNextModel(store.getState(), "a")).toEqual(opus)
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 1000, "idle", { model: next, revision: 2 }),
    })
    await settle()
    expect(selectNextModel(store.getState(), "a")).toEqual(next)
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
    expect(requestId).not.toBe("")
  })

  it("waits for the receipt when the frame arrives first, capturing immutable input", async () => {
    const { source, store } = await ready()
    let done!: (value: ModelReceipt) => void
    let requestId = ""
    source.setModel = vi.fn((_id, model, id) => {
      requestId = id
      expect(model).toEqual(next)
      return new Promise<ModelReceipt>((resolve) => {
        done = resolve
      })
    })
    const mutable = { ...next }
    const command = changeModel({ sessionId: "a", model: mutable })
    mutable.modelId = "mutated"
    const pending = store.dispatch(command)
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 1000, "idle", { model: next, revision: 2 }),
    })
    await settle()
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    done(receipt(requestId))
    await pending
    await settle()
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
  })

  it("retains draft and sends nothing while a model command or source recovery is pending", async () => {
    const { source, store } = await ready()
    source.hold("setModel")
    store.dispatch(setComposerText({ sessionId: "a", text: "unsent draft" }))
    const change = store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "a", text: "unsent draft", initiator: "person" }),
      ),
    ).toBe("not-asked")
    expect(store.getState().workspace.composerText.a).toBe("unsent draft")
    expect(source.calls.some((call) => call[0] === "send")).toBe(false)
    await source.release("setModel")
    await change
    source.emit({
      kind: "transcript",
      transcript: {
        ...emptyTranscript("a"),
        revision: 2,
        modelControl: {
          pending: {
            requestId: "other",
            requestedModel: next.modelId,
            status: "recovery_required",
          },
        },
      },
    })
    await settle()
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "a", text: "unsent draft", initiator: "person" }),
      ),
    ).toBe("not-asked")
  })

  it("does not resend on text revisions or reinterpret a refusal as Busy", async () => {
    const { source, store } = await ready()
    source.refuse("setModel", "unavailable")
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(store.getState().workspace.modelChoices.a).toMatchObject({
      state: "refused",
      reason: "unavailable",
    })
    source.emit({
      kind: "transcript",
      transcript: { ...emptyTranscript("a"), revision: 2 },
    })
    await settle()
    expect(source.calls.filter((call) => call[0] === "setModel")).toHaveLength(1)
    expect(selectNextModel(store.getState(), "a")).toEqual(opus)
    expect(modelChanging(store.getState().workspace, "a")).toBe(false)
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

  it("rejects a mismatched receipt without an applied model", async () => {
    const { source, store } = await ready()
    source.setModel = async () => receipt("wrong")
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(store.getState().workspace.modelChoices.a?.state).toBe("recovery_required")
    expect(selectNextModel(store.getState(), "a")).toEqual(opus)
  })

  it("asks once while held, then allows an explicit new choice after refusal", async () => {
    const { source, store } = await ready()
    source.hold("setModel")
    source.refuse("setModel", "not-supported")
    const first = store.dispatch(changeModel({ sessionId: "a", model: next }))
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(source.calls.filter((call) => call[0] === "setModel")).toHaveLength(1)
    await source.release("setModel")
    await first
    source.refuse("setModel", undefined)
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(source.calls.filter((call) => call[0] === "setModel")).toHaveLength(2)
    expect(selectNextModel(store.getState(), "a")).toEqual(next)
  })
  it("forgets a removed target and ignores its late receipt", async () => {
    const { source, store } = await ready()
    source.hold("setModel")
    const pending = store.dispatch(changeModel({ sessionId: "a", model: next }))
    await store.dispatch(archiveSession({ sessionId: "a", initiator: "person" }))
    await settle()
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
    await source.release("setModel")
    await pending
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
  })

  it("keeps original review answers and Stop independent of held model work", async () => {
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
    source.hold("setModel")
    const pending = store.dispatch(changeModel({ sessionId: "a", model: next }))
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
    expect(source.calls.some((call) => call[0] === "approve")).toBe(true)
    expect(source.calls.some((call) => call[0] === "stop")).toBe(true)
    await source.release("setModel")
    await pending
  })
  it("keeps a matching receipt held while the source says recovery is pending", async () => {
    const { source, store } = await ready()
    source.setModel = async () => {
      throw new WorkspaceModelError("unavailable", true)
    }
    source.emit({
      kind: "transcript",
      transcript: {
        ...emptyTranscript("a"),
        revision: 2,
        modelControl: {
          pending: {
            requestId: "source",
            requestedModel: next.modelId,
            status: "recovery_required",
          },
        },
      },
    })
    // A deliberate pick may reconcile recovery, but cannot clear the send fence.
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(store.getState().workspace.modelChoices.a?.state).toBe("recovery_required")
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    source.setModel = async (_id, _model, id) => receipt(id)
    source.emit({
      kind: "transcript",
      transcript: { ...emptyTranscript("a"), revision: 3 },
    })
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    source.emit({
      kind: "transcript",
      transcript: {
        ...emptyTranscript("a"),
        revision: 4,
        modelControl: {
          pending: {
            requestId: "source",
            requestedModel: next.modelId,
            status: "recovery_required",
          },
        },
      },
    })
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 1000, "idle", { model: next, revision: 2 }),
    })
    await settle()
    expect(store.getState().workspace.modelChoices.a?.state).toBe("acknowledged")
    source.emit({
      kind: "transcript",
      transcript: { ...emptyTranscript("a"), revision: 5 },
    })
    await settle()
    expect(store.getState().workspace.modelChoices.a).toBeUndefined()
  })
  it("fences Send after an uncertain model result with no recovered source frame", async () => {
    const { source, store } = await ready()
    source.setModel = async () => {
      throw new WorkspaceModelError("unavailable", true)
    }
    store.dispatch(setComposerText({ sessionId: "a", text: "keep this" }))
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "a", text: "keep this", initiator: "person" }),
      ),
    ).toBe("not-asked")
    expect(store.getState().workspace.composerText.a).toBe("keep this")
    expect(source.calls.some((call) => call[0] === "send")).toBe(false)
  })
  it("does not retry a failed outbox message while model recovery is unresolved", async () => {
    const { source, store } = await ready()
    source.refuse("send", "unavailable")
    await store.dispatch(
      sendMessage({ sessionId: "a", text: "retained", initiator: "person" }),
    )
    const failed = store.getState().workspace.outbox.a![0]!
    source.refuse("send", undefined)
    source.setModel = async () => {
      throw new WorkspaceModelError("unavailable", true)
    }
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    const sends = source.calls.filter((call) => call[0] === "send").length
    expect(
      await store.dispatch(
        resendMessage({ sessionId: "a", messageId: failed.id, initiator: "person" }),
      ),
    ).toBe("not-asked")
    expect(source.calls.filter((call) => call[0] === "send")).toHaveLength(sends)
    expect(store.getState().workspace.outbox.a).toEqual([failed])
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 1000, "idle", { model: opus, revision: 2 }),
    })
    await settle()
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 1000, "idle", { model: next, revision: 3 }),
    })
    await settle()
    expect(modelChanging(store.getState().workspace, "a")).toBe(false)
    expect(
      await store.dispatch(
        resendMessage({ sessionId: "a", messageId: failed.id, initiator: "person" }),
      ),
    ).toBe("sent")
  })
  it.each([false, true])(
    "settles a reconciled receipt after newer source B (intermediate A observed: %s)",
    async (includeA) => {
      const { source, store } = await ready()
      const other = { provider: opus.provider, modelId: "claude-fable-5-1" }
      source.setModel = async (_id, _model, id) => {
        if (includeA)
          source.emit({
            kind: "session",
            session: summary("a", "desktop", 1000, "idle", { model: next, revision: 2 }),
          })
        source.emit({
          kind: "session",
          session: summary("a", "desktop", 1000, "idle", { model: other, revision: 3 }),
        })
        return { ...receipt(id), sourceReconciled: true }
      }
      await store.dispatch(changeModel({ sessionId: "a", model: next }))
      await settle()
      expect(selectNextModel(store.getState(), "a")).toEqual(other)
      expect(modelChanging(store.getState().workspace, "a")).toBe(false)
      expect(store.getState().workspace.modelChoices.a).toBeUndefined()
      await store.dispatch(
        sendMessage({ sessionId: "a", text: "after source B", initiator: "person" }),
      )
      expect(source.calls.find((call) => call[0] === "send")?.[1]).toMatchObject({
        model: other,
        text: "after source B",
      })
    },
  )
  it("keeps a completed ACK without reconciliation fenced but permits deliberate recovery", async () => {
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
    store.dispatch(setComposerText({ sessionId: "a", text: "still mine" }))
    await store.dispatch(changeModel({ sessionId: "a", model: next }))
    expect(modelChanging(store.getState().workspace, "a")).toBe(true)
    expect(modelRequesting(store.getState().workspace, "a")).toBe(false)
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "a", text: "still mine", initiator: "person" }),
      ),
    ).toBe("not-asked")
    await store.dispatch(changeModel({ sessionId: "a", model: opus }))
    expect(calls).toBe(2)
    expect(modelChanging(store.getState().workspace, "a")).toBe(false)
    expect(store.getState().workspace.composerText.a).toBe("still mine")
  })
})
