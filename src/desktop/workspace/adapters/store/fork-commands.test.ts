/** Forking a session: the source's fork, then the new session opened in place. */
import { expect, it } from "vitest"
import { fakeSource, settle, testStore } from "../../testing"
import { paneShowingSession } from "../../application/workspace-state"
import { handoffLine } from "../../model/handoff"
import { followWorkspace, forkSession, loadWorkspace, newSession } from "./commands"

async function ready() {
  const source = fakeSource()
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  store.dispatch(followWorkspace())
  await settle()
  return { source, store }
}

it("forks a session the source has spoken of and opens the new one, and asks nothing for a draft", async () => {
  const { source, store } = await ready()
  await expect(
    store.dispatch(forkSession({ sessionId: "a", initiator: "person" })),
  ).resolves.toBe("sent")
  expect(source.calls).toContainEqual(["fork", "a", "person"])
  await settle()
  const panes = store.getState().workspace.panes
  expect(panes && paneShowingSession(panes, "a-fork")).toBeDefined()
  const draft = store.dispatch(newSession())!
  await expect(
    store.dispatch(forkSession({ sessionId: draft, initiator: "person" })),
  ).resolves.toBe("not-asked")
})

it("says a switch by the agent and model it went to, and a fork by what it left out", () => {
  expect(
    handoffLine({
      kind: "switch",
      from: { agent: "claude", model: "claude-opus-5" },
      to: { agent: "codex", model: "unlisted-model" },
    }),
  ).toEqual({ label: "Switched to Codex", meta: "unlisted-model" })
  expect(
    handoffLine({ kind: "switch", from: { model: "a" }, to: { model: "b" } }).label,
  ).toBe("Switched to another agent")
  expect(handoffLine({ kind: "fork", omittedTurns: 0 })).toEqual({
    label: "Forked from another chat",
  })
  expect(handoffLine({ kind: "fork", omittedTurns: 1 }).meta).toBe(
    "1 earlier turn not shown",
  )
})
