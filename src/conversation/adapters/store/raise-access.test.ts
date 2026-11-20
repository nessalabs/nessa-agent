import { expect, it } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { textContent, type ApprovalMode } from "../../model"
import { scenarioEffects } from "../scenario/effects"
import { raiseAccess, sendDraft } from "./slice"

/**
 * An access request's Approve is sent only after the access really changed,
 * so `raiseAccess` must never say "taken" for a change it let go without
 * asking: a tab not begun, or one with another control on its way.
 */
it("does not call a change taken when it was never asked for", async () => {
  const asked: ApprovalMode[] = []
  let finish!: () => void
  const effects = scenarioEffects("echo")
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        setApprovalMode: (_id: string, mode: ApprovalMode) => {
          asked.push(mode)
          return new Promise<ApprovalMode>((resolve) => (finish = () => resolve(mode)))
        },
      },
    }),
  )
  // Not begun: nothing to change yet.
  expect(await store.dispatch(raiseAccess({ id: "c0", mode: "full" })).unwrap()).toBe(
    false,
  )
  await store.dispatch(sendDraft({ content: textContent("hello") }))
  const first = store.dispatch(raiseAccess({ id: "c0", mode: "full" }))
  await Promise.resolve()
  await Promise.resolve()
  // Another control is on its way, so this one is let go, and says so.
  expect(await store.dispatch(raiseAccess({ id: "c0", mode: "auto" })).unwrap()).toBe(
    false,
  )
  await new Promise((resolve) => setTimeout(resolve, 0))
  finish()
  expect(await first.unwrap()).toBe(true)
  expect(asked).toEqual(["full"])
})
