/** The overview's commands, through the one command list. */
import { describe, expect, it } from "vitest"

import { panesOf } from "../../../split-panes/model/pane-layout"
import { emptyTranscript } from "../../model/transcript"

import {
  fakeSource,
  keptFilter,
  outboxOf,
  ready,
  settle,
  summary,
  testIndex,
  testStore,
} from "../../testing"
import { retention } from "../../model/retention"
import { quietOf } from "../../model/overview/agents-glance"
import { selectGlance } from "./selectors"
import {
  approve,
  archiveSession,
  closePane,
  deny,
  followWorkspace,
  newSession,
  nudgePane,
  openBeside,
  openChannel,
  openSession,
  resizePanes,
  selectChannel,
  sendMessage,
  showContent,
  focusPane,
  equalizePanes,
  filterOverview,
  selectInOverview,
  showOverviewGroup,
  toggleSessionList,
  toggleSidebar,
} from "./commands"

describe("the Agents overview is a place in the sidebar, left by going anywhere else", () => {
  const ways = {
    "the channel already listed": selectChannel({ channelId: "desktop" }),
    "another channel": selectChannel({ channelId: "gateway" }),
    "a session": openSession({ sessionId: "c" }),
    "a session beside": openBeside({ sessionId: "c" }),
    "a channel from the tree": openChannel({ channelId: "gateway" }),
    "a new session": newSession(),
    "another pane": focusPane({ pane: 1 }),
    "closing a pane": closePane(),
    "moving a pane": nudgePane({ pane: 1, direction: "right" }),
    "evening the panes out": equalizePanes(),
    "resizing panes": resizePanes({
      edge: { axis: "x", column: 0 },
      fraction: 0.6,
      pair: 1,
    }),
  }
  for (const [way, action] of Object.entries(ways))
    it(`goes back to the panes on ${way}`, async () => {
      const { store } = await ready()
      // Two panes, unevenly: every change to them changes something.
      store.dispatch(openBeside({ sessionId: "c", side: "right" }))
      store.dispatch(
        resizePanes({ edge: { axis: "x", column: 0 }, fraction: 0.4, pair: 1 }),
      )
      store.dispatch(showContent({ content: "agents" }))
      store.dispatch(showContent({ content: "agents" }))
      expect(store.getState().workspace.content).toBe("agents")
      store.dispatch(action as Parameters<typeof store.dispatch>[0])
      expect(store.getState().workspace.content).toBe("panes")
    })

  it("stays when a change to the panes changes nothing: a move at the edge, a close of no pane", async () => {
    const { store } = await ready()
    const panes = store.getState().workspace.panes
    if (!panes) throw new Error("no panes")
    store.dispatch(showContent({ content: "agents" }))
    const edge = panesOf(panes)[0].key
    store.dispatch(nudgePane({ pane: edge, direction: "left" }))
    store.dispatch(closePane({ pane: 999 }))
    expect(store.getState().workspace.panes).toBe(panes)
    expect(store.getState().workspace.content).toBe("agents")
    // Going to the pane already focused is still going there.
    store.dispatch(focusPane({ pane: panes.focused }))
    expect(store.getState().workspace.content).toBe("panes")
  })

  it("stays over what only rearranges the window", async () => {
    const { store } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(toggleSidebar())
    store.dispatch(toggleSessionList())
    expect(store.getState().workspace.content).toBe("agents")
  })
})

describe("the Agents overview answers and reads what it shows, as a pane does", () => {
  const waitingOn = (source = fakeSource()) => {
    source.transcripts.set("b", {
      ...emptyTranscript("b"),
      revision: 1,
      approvals: [
        {
          id: "ap",
          command: "cargo test",
          reason: "Runs tests.",
          origin: { kind: "agent" },
          options: [],
          ask: "tool",
        },
      ],
    })
    return source
  }

  it("reads the conversation of a session waiting on the person once the overview opens, without marking it read", async () => {
    const { store, source } = await ready(waitingOn())
    expect(source.calls).not.toContainEqual(["transcript", "b"])
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    expect(source.calls).toContainEqual(["transcript", "b"])
    expect(store.getState().workspace.transcripts.b?.approvals[0]?.id).toBe("ap")
    // A peek is not reading the session — not when it opens, nor when the session changes.
    expect(store.getState().workspace.sessions.b.unread).toBe(true)
    store.dispatch(followWorkspace())
    source.emit({
      kind: "session",
      session: summary("b", "desktop", 250, "needs-you", { unread: true, revision: 2 }),
    })
    await settle()
    expect(store.getState().workspace.sessions.b.unread).toBe(true)
    expect(source.calls).not.toContainEqual(["markRead", "b"])
  })

  it("answers an approval the open overview shows in no pane, as the person, through the one answer", async () => {
    const { store, source } = await ready(waitingOn())
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    source.hold("approve")
    const answer = store.dispatch(
      approve({
        sessionId: "b",
        approvalId: "ap",
        initiator: "person",
        optionId: "once",
      }),
    )
    // A second answer, from a pane or the overview, waits on the first.
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "person", optionId: "deny" }),
      ),
    ).toBe("answering")
    await source.release("approve")
    expect(await answer).toBe("sent")
    expect(source.calls).toContainEqual(["approve", "b", "ap", "once", "person", "once"])
    expect(source.calls.filter((call) => call[0] === "deny")).toEqual([])
    expect(store.getState().workspace.content).toBe("agents")
  })

  it("says an approval was not asked when neither a pane nor the open overview shows it", async () => {
    const { store, source } = await ready(waitingOn())
    // Open, but listing only sessions under a tag this one does not carry.
    store.dispatch(
      filterOverview({ filter: { scope: "all", range: "any", tags: ["ui"] } }),
    )
    store.dispatch(showContent({ content: "agents" }))
    expect(store.getState().workspace.overview.selected).not.toBe("b")
    await settle()
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "agent",
          optionId: "once",
        }),
      ),
    ).toBe("not-asked")
    store.dispatch(showContent({ content: "panes" }))
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "agent",
          optionId: "once",
        }),
      ),
    ).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  })

  it("keeps a refused answer's reason, and answers again, from the overview", async () => {
    const { store, source } = await ready(waitingOn())
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    source.refuse("approve", "not-waiting")
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "person",
          optionId: "once",
        }),
      ),
    ).toBe("refused")
    source.refuse("approve", "unavailable")
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "person",
          optionId: "once",
        }),
      ),
    ).toBe("unknown")
    expect(store.getState().workspace.answers.b?.[0]?.failure).toBe("unavailable")
    source.refuse("approve", undefined)
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "person",
          optionId: "once",
        }),
      ),
    ).toBe("sent")
  })

  it("keeps an answer on its way when the overview closes, and lets it go on the conversation that no longer asks", async () => {
    const { store, source } = await ready(waitingOn())
    store.dispatch(followWorkspace())
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    source.hold("approve")
    const answer = store.dispatch(
      approve({
        sessionId: "b",
        approvalId: "ap",
        initiator: "person",
        optionId: "once",
      }),
    )
    store.dispatch(showContent({ content: "panes" }))
    expect(store.getState().workspace.answers.b?.[0]?.approvalId).toBe("ap")
    await source.release("approve")
    expect(await answer).toBe("sent")
    await settle()
    expect(store.getState().workspace.answers.b).toBeUndefined()
  })

  it("sends a reply from the overview to a waiting session no pane shows, as the person, and says when it is not sent", async () => {
    const { store, source } = await ready(waitingOn())
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    source.hold("send")
    const reply = store.dispatch(
      sendMessage({ sessionId: "b", text: "Use the beta profile.", initiator: "person" }),
    )
    expect(outboxOf(store, "b").map((message) => message.delivery?.state)).toEqual([
      "sending",
    ])
    await source.release("send")
    expect(await reply).toBe("sent")
    expect(source.calls.find((call) => call[0] === "send")?.[1]).toMatchObject({
      sessionId: "b",
      initiator: "person",
    })
    source.refuse("send", "unavailable")
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "b", text: "And again.", initiator: "person" }),
      ),
    ).toBe("unknown")
    expect(outboxOf(store, "b").at(-1)?.delivery).toMatchObject({
      state: "failed",
      reason: "unavailable",
    })
  })

  it("reads what a session chosen in the overview says, and lets it go when it is removed", async () => {
    const { store, source } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(selectInOverview({ sessionId: "d" }))
    await settle()
    expect(source.calls).toContainEqual(["transcript", "d"])
    // A session not listed is not chosen.
    store.dispatch(selectInOverview({ sessionId: "nowhere" }))
    expect(store.getState().workspace.overview.selected).toBe("d")
    store.dispatch(followWorkspace())
    await store.dispatch(archiveSession({ sessionId: "d", initiator: "person" }))
    // Gone, it is not chosen: the first the overview lists is, and is read.
    const chosen = store.getState().workspace.overview.selected
    expect(chosen).not.toBe("d")
    expect(chosen).not.toBeNull()
    await settle()
    expect(source.calls).toContainEqual(["transcript", chosen])
  })

  it("chooses the first it lists on opening, so what the peek shows is what the window reads", async () => {
    const { store, source } = await ready()
    expect(store.getState().workspace.overview.selected).toBeNull()
    store.dispatch(showContent({ content: "agents" }))
    const chosen = store.getState().workspace.overview.selected
    expect(chosen).not.toBeNull()
    await settle()
    expect(source.calls).toContainEqual(["transcript", chosen])
  })

  it("chooses nothing while it lists nothing", async () => {
    const { store } = await ready()
    store.dispatch(
      filterOverview({ filter: { scope: "all", range: "any", tags: ["nowhere"] } }),
    )
    store.dispatch(showContent({ content: "agents" }))
    expect(store.getState().workspace.overview.selected).toBeNull()
  })

  it("reads and keeps the conversations of at most so many waiting sessions, and a chosen one past them", async () => {
    const many = Array.from({ length: retention.overviewConversations + 6 }, (_, at) =>
      summary(`w${at}`, "desktop", 500 + at, "needs-you"),
    )
    const index = testIndex()
    const source = fakeSource({ ...index, sessions: [...index.sessions, ...many] })
    const { store } = await ready(source)
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    const read = () =>
      new Set(
        source.calls.filter((call) => call[0] === "transcript").map((call) => call[1]),
      )
    // The most recently active, "b" and the oldest waiting left out.
    expect(read().has("w0")).toBe(false)
    expect(read().has(`w${many.length - 1}`)).toBe(true)
    const kept = Object.keys(store.getState().workspace.transcripts)
    expect(kept.length).toBeLessThanOrEqual(
      retention.overviewConversations + retention.unshownConversations + 1,
    )
    store.dispatch(selectInOverview({ sessionId: "w0" }))
    await settle()
    expect(read().has("w0")).toBe(true)
    expect(store.getState().workspace.transcripts.w0).toBeDefined()
  })

  it("keeps a failed read while the overview shows the session, and reads it afresh once shown again", async () => {
    const source = waitingOn()
    const { store } = await ready(source)
    source.refuse("transcript", "unavailable")
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    expect(store.getState().workspace.transcriptFailures.b).toBe("unavailable")
    store.dispatch(showContent({ content: "panes" }))
    expect(store.getState().workspace.transcriptFailures.b).toBeUndefined()
    source.refuse("transcript", undefined)
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    expect(store.getState().workspace.transcripts.b?.approvals[0]?.id).toBe("ap")
  })
})

describe("the overview's filter is the store's, kept between launches", () => {
  it("starts from what was kept, and keeps each change", async () => {
    const kept = keptFilter({ scope: "all", range: "week", tags: [] })
    const store = testStore(fakeSource(), undefined, kept)
    expect(store.getState().workspace.overview.filter).toEqual({
      scope: "all",
      range: "week",
      tags: [],
    })
    store.dispatch(
      filterOverview({ filter: { scope: "ongoing", range: "any", tags: [] } }),
    )
    expect(kept.writes).toEqual([{ scope: "ongoing", range: "any", tags: [] }])
  })
})

describe("the overview shows one group alone (ADR 238, the group's table)", () => {
  const waitingOn = (source = fakeSource()) => {
    source.transcripts.set("b", {
      ...emptyTranscript("b"),
      revision: 1,
      approvals: [
        {
          id: "ap",
          command: "cargo test",
          reason: "Runs tests.",
          origin: { kind: "agent" },
          options: [],
          ask: "tool",
        },
      ],
    })
    return source
  }
  const overview = (store: ReturnType<typeof testStore>) =>
    store.getState().workspace.overview

  it("shows a group, choosing the first it lists when the chosen session is outside it", async () => {
    const { store } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    // Opening chose the first listed: "b", which waits on the person.
    expect(overview(store).selected).toBe("b")
    store.dispatch(showOverviewGroup({ group: "working" }))
    expect(overview(store).group).toBe("working")
    expect(overview(store).selected).toBe("a")
  })

  it("keeps the chosen session when the group holds it", async () => {
    const { store } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(showOverviewGroup({ group: "needsYou" }))
    expect(overview(store).selected).toBe("b")
  })

  it("shows every group again on null, keeping the chosen session", async () => {
    const { store } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(showOverviewGroup({ group: "working" }))
    store.dispatch(showOverviewGroup({ group: null }))
    expect(overview(store)).toMatchObject({ group: null, selected: "a" })
  })

  it("changes nothing when the group shown is chosen again", async () => {
    const { store } = await ready()
    store.dispatch(showOverviewGroup({ group: "earlier" }))
    const before = store.getState().workspace
    store.dispatch(showOverviewGroup({ group: "earlier" }))
    expect(store.getState().workspace).toBe(before)
  })

  it("chooses nothing while the group lists nothing", async () => {
    const { store } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    // Ongoing lists no idle session: nothing finished is listed.
    store.dispatch(showOverviewGroup({ group: "finished" }))
    expect(overview(store).selected).toBeNull()
  })

  it("says the group lists nothing once its last session leaves it, and keeps the one looked at chosen under its new heading", async () => {
    const { store, source } = await ready()
    store.dispatch(followWorkspace())
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(showOverviewGroup({ group: "needsYou" }))
    expect(overview(store).selected).toBe("b")
    // Answered elsewhere: the source moves it on to working.
    source.emit({
      kind: "session",
      session: summary("b", "desktop", 500, "running", { unread: true, revision: 2 }),
    })
    await settle()
    expect(overview(store)).toMatchObject({ group: "needsYou", selected: "b" })
    const glance = selectGlance(store.getState(), [], 1000)
    expect(glance.needsYou).toEqual([])
    expect(glance.working).toEqual(["b"])
    expect(glance.counts.needsYou).toBe(0)
    expect(quietOf(glance)).toBe("needsYou")
  })

  it("reads the same glance while the list, the filter and the clock are unchanged", async () => {
    const { store } = await ready()
    const state = store.getState()
    const held: readonly [] = []
    expect(selectGlance(state, held, 1000)).toBe(selectGlance(state, held, 1000))
  })

  it("chooses nothing once the group's last session is removed", async () => {
    const { store, source } = await ready()
    store.dispatch(followWorkspace())
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(showOverviewGroup({ group: "needsYou" }))
    source.emit({ kind: "session-removed", sessionId: "b", revision: 2 })
    await settle()
    expect(overview(store)).toMatchObject({ group: "needsYou", selected: null })
    expect(quietOf(selectGlance(store.getState(), [], 1000))).toBe("needsYou")
  })

  it("keeps the group through a change of filter, and while the overview is closed and opened again", async () => {
    const { store } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(showOverviewGroup({ group: "working" }))
    store.dispatch(filterOverview({ filter: { scope: "all", range: "week", tags: [] } }))
    store.dispatch(showContent({ content: "panes" }))
    store.dispatch(showContent({ content: "agents" }))
    expect(overview(store).group).toBe("working")
  })

  it("never keeps the group between launches: a window opens on every group, and only the filter is written", async () => {
    const kept = keptFilter({ scope: "all", range: "any", tags: [] })
    const store = testStore(fakeSource(), undefined, kept)
    expect(overview(store).group).toBeNull()
    store.dispatch(showOverviewGroup({ group: "earlier" }))
    expect(kept.writes).toEqual([])
  })

  it("does not answer, nor read, a request its group leaves out, and answers it once the waiting are shown", async () => {
    const source = waitingOn()
    const { store } = await ready(source)
    store.dispatch(showOverviewGroup({ group: "working" }))
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    expect(overview(store).selected).toBe("a")
    expect(source.calls).not.toContainEqual(["transcript", "b"])
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "agent",
          optionId: "once",
        }),
      ),
    ).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
    store.dispatch(showOverviewGroup({ group: "needsYou" }))
    await settle()
    expect(source.calls).toContainEqual(["transcript", "b"])
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "agent",
          optionId: "once",
        }),
      ),
    ).toBe("sent")
  })
})
