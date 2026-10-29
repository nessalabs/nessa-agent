// @vitest-environment jsdom
import { act, StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { createPortal } from "react-dom"
import { PortalContainerProvider } from "@nessa-ui/react/lib/portal-container"
import { expect, it, vi } from "vitest"
import { VaultProvider, useVaultUnlock } from "./provider"
import { VaultRefusal, type VaultSource, type VaultState } from "../application/ports"
vi.mock("./dialog", () => ({
  VaultDialog: ({
    view,
    container,
  }: {
    view: { kind: string }
    container: HTMLElement | null
  }) => createPortal(<div data-vault-dialog={view.kind} />, container ?? document.body),
}))
function ProtectedUse() {
  const ensure = useVaultUnlock()
  return (
    <button
      onClick={() => {
        void ensure()
      }}
    >
      Protected use
    </button>
  )
}
it("keeps the mounted dialog subscribed after StrictMode replay and a later transport failure", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  let state: VaultState | "failed" = "unlocked"
  const source: VaultSource = {
    status: vi.fn(async () => {
      if (state === "failed") throw new VaultRefusal("vault_unavailable")
      return state
    }),
    setup: async () => "unlocked",
    unlock: async () => "unlocked",
    lock: async () => "locked",
  }
  const container = document.createElement("div")
  document.body.append(container)
  const root = createRoot(container)
  try {
    await act(async () =>
      root.render(
        <StrictMode>
          <VaultProvider source={source} autoUnlock>
            <ProtectedUse />
          </VaultProvider>
        </StrictMode>,
      ),
    )
    expect(container.querySelector("[data-vault-dialog]")).toBeNull()
    state = "failed"
    await act(async () => container.querySelector("button")?.click())
    expect(
      document.body.querySelector('[data-vault-dialog="unavailable"]'),
    ).not.toBeNull()
  } finally {
    await act(async () => root.unmount())
    container.remove()
  }
})

it("places protected-use dialogs in their surface layer and cancels when that requester leaves", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const source: VaultSource = {
    status: async () => "locked",
    setup: async () => "unlocked",
    unlock: async () => "unlocked",
    lock: async () => "locked",
  }
  const container = document.createElement("div")
  document.body.append(container)
  const layer = document.createElement("div")
  document.body.append(layer)
  const root = createRoot(container)
  const view = (shown: boolean) => (
    <VaultProvider source={source}>
      <PortalContainerProvider container={layer}>
        {shown ? <ProtectedUse /> : null}
      </PortalContainerProvider>
    </VaultProvider>
  )
  try {
    await act(async () => root.render(view(true)))
    await act(async () => container.querySelector("button")?.click())
    expect(layer.querySelector('[data-vault-dialog="unlock"]')).not.toBeNull()
    await act(async () => root.render(view(false)))
    expect(document.body.querySelector("[data-vault-dialog]")).toBeNull()
  } finally {
    await act(async () => root.unmount())
    container.remove()
    layer.remove()
  }
})
