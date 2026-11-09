import { NessaRpcError, type ConnectionState, type WorkspaceWindow } from "@nessa/client"
import { afterEach, describe, expect, it, vi } from "vitest"
import { panesOf } from "../../../split-panes/model/pane-layout"
import {
  closePanelTab,
  openWidgetInPanel,
  showPanelTab,
} from "../../application/usecases/chat-panel"
import { focusPane, openBeside } from "../../application/usecases/panes"
import { indexLoaded } from "../../application/usecases/updates"
import {
  arrangementShown,
  panelShown,
  shownWindow,
} from "../../application/usecases/window"
import { initialWorkspace } from "../../application/workspace-state"
import type { ShownWindow } from "../../model/shown-window"
import { roomyGrid, shownBy, settle, testIndex } from "../../testing"
import { windowPublisher, type WindowPublishClient } from "./window-publisher"

const WINDOW = "7d9f2a4e-1b3c-4d5e-8f60-718293a4b5c6"
const id = "0b3b8f0e-6f43-4a51-9d55-0e7e4f7f8a11"

const window = (sessionId: string): ShownWindow => ({
  arrangement: "columns",
  sidebar: true,
  sessionList: false,
  showing: { kind: "panes" },
  panel: { open: false, tabs: [], front: null },
  columns: [
    {
      widthPercent: 100,
      panes: [
        { heightPercent: 100, focused: true, item: { kind: "conversation", sessionId } },
      ],
    },
  ],
})

/** A gateway client whose publications wait until the test lets them go. */
function fakeClient() {
  const sent: { windowId: string; revision: number; window?: WorkspaceWindow }[] = []
  const pending: { resolve: () => void; reject: (error: unknown) => void }[] = []
  let handler: ((state: ConnectionState) => void) | undefined
  const client: WindowPublishClient = {
    workspace: {
      publish: (windowId, revision, window) => {
        sent.push({ windowId, revision, window })
        return new Promise((resolve, reject) =>
          pending.push({ resolve: () => resolve({ revision, applied: true }), reject }),
        )
      },
    },
    onConnectionStateChange: (next) => {
      handler = next
      return () => (handler = undefined)
    },
  }
  return {
    client,
    sent,
    answer: async () => {
      pending.shift()?.resolve()
      await settle()
    },
    fail: async (error: unknown) => {
      pending.shift()?.reject(error)
      await settle()
    },
    connection: (status: ConnectionState["status"]) =>
      handler?.({ status } as ConnectionState),
  }
}

function publisherOver(fake: ReturnType<typeof fakeClient>) {
  const retries: (() => void)[] = []
  const publisher = windowPublisher({
    connected: async () => fake.client,
    after: (_ms, run) => {
      retries.push(run)
      return () => {}
    },
    retryMs: 1000,
    windowId: WINDOW,
  })
  return { publisher, retries }
}

afterEach(() => vi.restoreAllMocks())

describe("publishing what the window shows", () => {
  it("says it in the protocol's words, a conversation by its id", async () => {
    const fake = fakeClient()
    publisherOver(fake).publisher.publish(window(id))
    await settle()
    expect(fake.sent).toEqual([
      {
        windowId: WINDOW,
        revision: 1,
        window: {
          arrangement: "columns",
          sidebar: true,
          sessionList: false,
          showing: "panes",
          panel: { open: false, tabs: [] },
          columns: [
            {
              widthPercent: 100,
              panes: [
                {
                  heightPercent: 100,
                  focused: true,
                  item: { kind: "conversation", conversationId: id },
                },
              ],
            },
          ],
        },
      },
    ])
  })

  it("says the focused chat's panel through each of its states", async () => {
    const subagents = { plugin: "subagents", id: "chat" }
    const app = { plugin: "mcp:huddle", id: "call-1" }
    const notes = { plugin: "subagents", id: "other" }
    const loaded = arrangementShown(
      indexLoaded(initialWorkspace, { index: testIndex(), draftId: "unused", read: "r" }),
      { arrangement: "columns" },
    )
    const opened = panelShown(
      openWidgetInPanel(openWidgetInPanel(loaded, { widget: subagents }), {
        widget: app,
      }),
      { open: true },
    )
    const fake = fakeClient()
    const { publisher } = publisherOver(fake)
    const say = async (state: typeof opened) => {
      publisher.publish(shownWindow(state))
      await settle()
      await fake.answer()
      return fake.sent.at(-1)?.window?.panel
    }
    expect(await say(panelShown(opened, { open: false }))).toEqual({
      open: false,
      tabs: [
        { plugin: "subagents", id: "chat" },
        { plugin: "mcp:huddle", id: "call-1" },
      ],
      front: { plugin: "mcp:huddle", id: "call-1" },
    })
    expect(await say(opened)).toEqual({
      open: true,
      tabs: [
        { plugin: "subagents", id: "chat" },
        { plugin: "mcp:huddle", id: "call-1" },
      ],
      front: { plugin: "mcp:huddle", id: "call-1" },
    })
    expect(
      await say(showPanelTab(opened, { sessionId: "a", widget: subagents })),
    ).toEqual({
      open: true,
      tabs: [
        { plugin: "subagents", id: "chat" },
        { plugin: "mcp:huddle", id: "call-1" },
      ],
      front: { plugin: "subagents", id: "chat" },
    })
    const beside = openBeside(opened, { sessionId: "b", room: roomyGrid })
    const theirs = openWidgetInPanel(beside, { widget: notes, origin: "b" })
    expect(await say(theirs)).toEqual({
      open: true,
      tabs: [{ plugin: "subagents", id: "other" }],
      front: { plugin: "subagents", id: "other" },
    })
    const back = focusPane(
      theirs,
      panesOf(theirs.panes!).find((pane) => shownBy(pane) === "a")!.key,
    )
    const none = closePanelTab(
      closePanelTab(back, { sessionId: "a", widget: subagents }),
      { sessionId: "a", widget: app },
    )
    expect(await say(none)).toEqual({ open: true, tabs: [] })
  })

  it("says a session that is not a conversation as a draft, and nothing as a withdrawal", async () => {
    const fake = fakeClient()
    const { publisher } = publisherOver(fake)
    publisher.publish(window("sample"))
    await settle()
    expect(fake.sent[0].window?.columns[0].panes[0].item).toEqual({ kind: "draft" })
    await fake.answer()
    publisher.publish(null)
    await settle()
    expect(fake.sent[1]).toEqual({ windowId: WINDOW, revision: 2, window: undefined })
  })

  it("sends one at a time, the newest last, skipping what was replaced meanwhile", async () => {
    const fake = fakeClient()
    const { publisher } = publisherOver(fake)
    publisher.publish(window("one"))
    await settle()
    publisher.publish(window("two"))
    publisher.publish(window(id))
    await settle()
    expect(fake.sent).toHaveLength(1)
    await fake.answer()
    expect(fake.sent.map((s) => s.revision)).toEqual([1, 2])
    expect(fake.sent[1].window?.columns[0].panes[0].item).toEqual({
      kind: "conversation",
      conversationId: id,
    })
  })

  it("says it again when the connection comes back", async () => {
    const fake = fakeClient()
    publisherOver(fake).publisher.publish(window(id))
    await settle()
    await fake.answer()
    fake.connection("connected")
    await settle()
    expect(fake.sent.map((s) => s.revision)).toEqual([1, 2])
    expect(fake.sent[1].window).toEqual(fake.sent[0].window)
  })

  it("tries again after a failed send or a refusal for now, not one for good", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {})
    const fake = fakeClient()
    const { publisher, retries } = publisherOver(fake)
    publisher.publish(window(id))
    await settle()
    await fake.fail(new Error("socket closed"))
    expect(retries).toHaveLength(1)
    retries[0]()
    await settle()
    await fake.fail(new NessaRpcError("temporarily_unavailable", "Busy"))
    expect(retries).toHaveLength(2)
    retries[1]()
    await settle()
    expect(fake.sent.map((s) => s.revision)).toEqual([1, 2, 3])
    await fake.fail(new NessaRpcError("invalid_request", "No"))
    // Refused for good: what the gateway held before is withdrawn, and that is all.
    expect(fake.sent[3]).toEqual({ windowId: WINDOW, revision: 4, window: undefined })
    // The withdrawal fails for now, and is sent again, also on reconnecting.
    await fake.fail(new NessaRpcError("temporarily_unavailable", "Busy"))
    expect(retries).toHaveLength(3)
    fake.connection("connected")
    await settle()
    expect(fake.sent[4]).toEqual({ windowId: WINDOW, revision: 5, window: undefined })
    await fake.answer()
    expect(fake.sent).toHaveLength(5)
    expect(retries).toHaveLength(3)
    expect(console.warn).toHaveBeenCalledOnce()
  })

  it("sends a change that came while a refused publication was out, in its place", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {})
    const fake = fakeClient()
    const { publisher } = publisherOver(fake)
    publisher.publish(window("one"))
    await settle()
    publisher.publish(window(id))
    await fake.fail(new NessaRpcError("invalid_request", "No"))
    await fake.answer()
    expect(
      fake.sent.map((s) => [s.revision, s.window?.columns[0].panes[0].item]),
    ).toEqual([
      [1, { kind: "draft" }],
      [2, { kind: "conversation", conversationId: id }],
    ])
  })
})
