// @vitest-environment jsdom
/**
 * The composer's shield shows the access its owner gives it, says a refused
 * change, and is disabled where no owner keeps a choice.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { Composer, type ComposerAccess } from "./composer"

let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

async function shown(access: ComposerAccess | undefined) {
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Composer
        page={false}
        onPageChange={() => {}}
        text=""
        onTextChange={() => {}}
        model={undefined}
        access={access}
      />,
    ),
  )
  const shield = host.querySelector<HTMLButtonElement>("[data-access-mode]")
  return { root, shield }
}

it("shows the owner's access and the sentence for a refused change", async () => {
  const { root, shield } = await shown({
    value: "full-access",
    onChange: () => {},
    notice: {
      tone: "warning",
      text: "Access can’t change while the agent is working. Try again when it stops.",
    },
  })
  expect(shield?.getAttribute("data-access-mode")).toBe("full-access")
  expect(shield?.disabled).toBe(false)
  expect(host.querySelector('[role="status"]')?.textContent).toBe(
    "Access can’t change while the agent is working. Try again when it stops.",
  )
  await act(async () => root.unmount())
})

it("disables the shield, and says nothing, when no owner keeps the choice", async () => {
  const { root, shield } = await shown(undefined)
  expect(shield?.disabled).toBe(true)
  expect(host.querySelector('[role="status"]')).toBeNull()
  await act(async () => root.unmount())
})
