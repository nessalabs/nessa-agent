// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { VaultChangeDialog } from "./change-dialog"
let root: Root
let host: HTMLElement
beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})
afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})
async function fill(input: HTMLInputElement, value: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(
      input,
      value,
    )
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}
it("checks confirmation before dispatch and erases transient input on submission", async () => {
  const submit = vi.fn(async () => {})
  await act(async () =>
    root.render(
      <VaultChangeDialog
        view={{ kind: "form", busy: false }}
        container={null}
        onSubmit={submit}
        onClose={() => {}}
        onUnlock={() => {}}
      />,
    ),
  )
  const inputs = [
    ...document.querySelectorAll<HTMLInputElement>("[data-vault-change] input"),
  ]
  const form = document.querySelector("[data-vault-change] form")
  if (!form) throw new Error("Change form missing")
  await fill(inputs[0], "synthetic replacement phrase")
  await fill(inputs[1], "different synthetic phrase")
  await act(async () => {
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
  })
  expect(submit).not.toHaveBeenCalled()
  expect(
    document.querySelector('[data-vault-change] [role="alert"]')?.textContent,
  ).toContain("don't match")
  await fill(inputs[1], "synthetic replacement phrase")
  await act(async () => {
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
  })
  expect(submit).toHaveBeenCalledExactlyOnceWith("synthetic replacement phrase")
  expect(inputs.map((input) => input.value)).toEqual(["", ""])
})
it("unconfirmed recovery offers explicit unlock without a resubmission form", async () => {
  const submit = vi.fn(async () => {})
  const unlock = vi.fn()
  await act(async () =>
    root.render(
      <VaultChangeDialog
        view={{ kind: "finished", failure: "vault_change_not_confirmed" }}
        container={null}
        onSubmit={submit}
        onClose={() => {}}
        onUnlock={unlock}
      />,
    ),
  )
  const dialog = document.querySelector("[data-vault-change]")
  if (!dialog) throw new Error("Change result missing")
  expect(dialog.textContent).toContain("new passphrase first")
  expect(dialog.textContent).toContain("old passphrase")
  expect(dialog.querySelector("form")).toBeNull()
  const button = [...dialog.querySelectorAll("button")].find(
    (item) => item.textContent === "Unlock vault",
  )
  if (!button) throw new Error("Explicit unlock control missing")
  await act(async () => button.click())
  expect(unlock).toHaveBeenCalledOnce()
  expect(submit).not.toHaveBeenCalled()
})
