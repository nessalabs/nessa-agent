// @vitest-environment jsdom
/**
 * The composer's shield shows the access its owner gives it, says a refused
 * change as its own tooltip, and is disabled where no owner keeps a choice.
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

it("shows the owner's access, and says a refused change as its tooltip, marked", async () => {
  const words = "Access can’t change while the agent is working. Try again when it stops."
  const { root, shield } = await shown({
    value: "full-access",
    onChange: () => {},
    note: { tone: "warning", text: words },
  })
  expect(shield?.getAttribute("data-access-mode")).toBe("full-access")
  expect(shield?.disabled).toBe(false)
  expect(shield?.getAttribute("data-tooltip")).toBe(words)
  expect(shield?.getAttribute("aria-description")).toBe(words)
  expect(shield?.getAttribute("data-attention")).toBe("warning")
  // Said by the shield, not by a line in the composer.
  expect(host.querySelector('[role="status"]')).toBeNull()
  await act(async () => root.unmount())
})

it("says a neutral note without marking the shield", async () => {
  const { root, shield } = await shown({
    value: "ask-approval",
    onChange: () => {},
    note: { tone: "neutral", text: "Access is now Ask for approval." },
  })
  expect(shield?.getAttribute("data-tooltip")).toBe("Access is now Ask for approval.")
  expect(shield?.hasAttribute("data-attention")).toBe(false)
  await act(async () => root.unmount())
})

it("disables the shield, and says nothing, when no owner keeps the choice", async () => {
  const { root, shield } = await shown(undefined)
  expect(shield?.disabled).toBe(true)
  expect(host.querySelector('[role="status"]')).toBeNull()
  await act(async () => root.unmount())
})
