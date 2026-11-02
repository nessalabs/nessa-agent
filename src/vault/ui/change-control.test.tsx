// @vitest-environment jsdom
import { act, StrictMode, type ReactNode } from "react"
import { createRoot } from "react-dom/client"
import { createPortal, flushSync } from "react-dom"
import { expect, it, vi } from "vitest"
import { VaultProvider, useVaultChangeOwnership, useVaultUnlock } from "./provider"
import { VaultPassphraseControl } from "./change-control"
import { VaultRefusal, type VaultSource, type VaultState } from "../application/ports"
import type { VaultChangeView } from "../application/change-controller"
vi.mock("./dialog", () => ({
  VaultDialog: ({
    view,
    onSubmit,
  }: {
    view: { kind: string; busy?: boolean }
    onSubmit: (phrase: string) => Promise<void>
  }) => (
    <div data-vault-dialog={view.kind} data-busy={view.busy || undefined}>
      <button
        onClick={() => {
          void onSubmit("synthetic unlock phrase")
        }}
      >
        Submit normal unlock
      </button>
    </div>
  ),
}))
vi.mock("./change-dialog", () => ({
  VaultChangeDialog: ({
    view,
    container,
    onSubmit,
    onClose,
    onUnlock,
  }: {
    view: VaultChangeView
    container: HTMLElement | null
    onSubmit: (phrase: string) => Promise<void>
    onClose: () => void
    onUnlock: () => void
  }) =>
    createPortal(
      <div data-vault-change={view.kind}>
        {view.kind === "form" ? (
          <button
            onClick={() => {
              void onSubmit("synthetic replacement phrase")
            }}
          >
            Submit change
          </button>
        ) : null}
        <button onClick={onClose}>Close change</button>
        <button onClick={onUnlock}>Unlock vault</button>
      </div>,
      container ?? document.body,
    ),
}))
function ProtectedUse({ result }: { result: (value: boolean) => void }) {
  const ensure = useVaultUnlock()
  return (
    <button
      onClick={() => {
        void ensure().then(result)
      }}
    >
      Protected use
    </button>
  )
}
function button(text: string) {
  const found = [...document.querySelectorAll("button")].find(
    (item) => item.textContent === text,
  )
  if (!found) throw new Error(`Missing button ${text}`)
  return found
}
function fixture() {
  let state: VaultState = "unlocked"
  const source: VaultSource = {
    status: vi.fn(async () => state),
    setup: async () => "unlocked",
    unlock: async () => "unlocked",
    lock: async () => "locked",
    changePassphrase: vi.fn(async () => {
      state = "locked"
      throw new VaultRefusal("vault_change_not_confirmed")
    }),
  }
  return {
    source,
    lock: () => {
      state = "locked"
    },
  }
}
it("keeps change recovery foreground after StrictMode replay, then explicitly opens unlock", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const { source } = fixture()
  const result = vi.fn()
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  try {
    await act(async () =>
      root.render(
        <StrictMode>
          <VaultProvider source={source}>
            <VaultPassphraseControl />
            <ProtectedUse result={result} />
          </VaultProvider>
        </StrictMode>,
      ),
    )
    await act(async () => button("Change passphrase").click())
    expect(document.querySelector('[data-vault-change="form"]')).not.toBeNull()
    await act(async () => button("Submit change").click())
    expect(document.querySelector('[data-vault-change="finished"]')).not.toBeNull()
    await act(async () => {
      button("Protected use").click()
      window.dispatchEvent(new Event("focus"))
    })
    expect(result).toHaveBeenLastCalledWith(false)
    expect(document.querySelector("[data-vault-dialog]")).toBeNull()
    expect(source.status).toHaveBeenCalledTimes(1)
    expect(source.changePassphrase).toHaveBeenCalledTimes(1)
    await act(async () => button("Unlock vault").click())
    expect(document.querySelector("[data-vault-change]")).toBeNull()
    expect(document.querySelector('[data-vault-dialog="unlock"]')).not.toBeNull()
    expect(source.status).toHaveBeenCalledTimes(2)
    expect(source.changePassphrase).toHaveBeenCalledTimes(1)
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})
it("releases foreground ownership on requester unmount without queueing an unlock", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const { source, lock } = fixture()
  const result = vi.fn()
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const view = (children: ReactNode) => (
    <VaultProvider source={source}>
      {children}
      <ProtectedUse result={result} />
    </VaultProvider>
  )
  try {
    await act(async () => root.render(view(<VaultPassphraseControl />)))
    await act(async () => button("Change passphrase").click())
    lock()
    await act(async () => button("Protected use").click())
    expect(result).toHaveBeenLastCalledWith(false)
    await act(async () => root.render(view(null)))
    expect(document.querySelector("[data-vault-dialog]")).toBeNull()
    expect(source.status).toHaveBeenCalledTimes(1)
    await act(async () => button("Protected use").click())
    expect(document.querySelector('[data-vault-dialog="unlock"]')).not.toBeNull()
    expect(source.status).toHaveBeenCalledTimes(2)
    expect(source.changePassphrase).not.toHaveBeenCalled()
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})

it("releases the old foreground lease when its source is replaced", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const previous = fixture()
  const next = fixture()
  next.lock()
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const result = vi.fn()
  const view = (source: VaultSource) => (
    <VaultProvider source={source}>
      <VaultPassphraseControl />
      <ProtectedUse result={result} />
    </VaultProvider>
  )
  try {
    await act(async () => root.render(view(previous.source)))
    await act(async () => button("Change passphrase").click())
    expect(document.querySelector('[data-vault-change="form"]')).not.toBeNull()
    await act(async () => root.render(view(next.source)))
    expect(document.querySelector("[data-vault-change]")).toBeNull()
    await act(async () => button("Protected use").click())
    expect(document.querySelector('[data-vault-dialog="unlock"]')).not.toBeNull()
    expect(previous.source.changePassphrase).not.toHaveBeenCalled()
    expect(next.source.changePassphrase).not.toHaveBeenCalled()
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})

it("rejects an old source completion queued across replacement before foreground acquisition", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const previous = fixture()
  const next = fixture()
  next.lock()
  let resolve!: (state: VaultState) => void
  previous.source.status = vi.fn(
    () =>
      new Promise<VaultState>((done) => {
        resolve = done
      }),
  )
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const result = vi.fn()
  const view = (source: VaultSource) => (
    <VaultProvider source={source}>
      <VaultPassphraseControl />
      <ProtectedUse result={result} />
    </VaultProvider>
  )
  try {
    await act(async () => root.render(view(previous.source)))
    await act(async () => button("Change passphrase").click())
    await act(async () => {
      resolve("unlocked")
      queueMicrotask(() => flushSync(() => root.render(view(next.source))))
    })
    expect(document.querySelector("[data-vault-change]")).toBeNull()
    await act(async () => button("Protected use").click())
    expect(document.querySelector('[data-vault-dialog="unlock"]')).not.toBeNull()
    expect(next.source.status).toHaveBeenCalledTimes(1)
    expect(previous.source.changePassphrase).not.toHaveBeenCalled()
    expect(next.source.changePassphrase).not.toHaveBeenCalled()
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})

function ForegroundProbe({
  result,
  label = "foreground",
}: {
  result: (accepted: boolean) => void
  label?: string
}) {
  const ownership = useVaultChangeOwnership()
  return (
    <>
      <button onClick={() => result(ownership.begin())}>Begin {label}</button>
      <button onClick={ownership.end}>End {label}</button>
    </>
  )
}
it("cancels a nonbusy delayed normal read before acquiring foreground ownership", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const { source } = fixture()
  let resolve!: (state: VaultState) => void
  source.status = vi.fn(
    () =>
      new Promise<VaultState>((done) => {
        resolve = done
      }),
  )
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const begun = vi.fn()
  const result = vi.fn()
  try {
    await act(async () =>
      root.render(
        <VaultProvider source={source}>
          <ForegroundProbe result={begun} />
          <ProtectedUse result={result} />
        </VaultProvider>,
      ),
    )
    await act(async () => button("Protected use").click())
    await act(async () => button("Begin foreground").click())
    expect(begun).toHaveBeenLastCalledWith(true)
    expect(result).toHaveBeenLastCalledWith(false)
    await act(async () => {
      resolve("locked")
      window.dispatchEvent(new Event("focus"))
    })
    expect(document.querySelector("[data-vault-dialog]")).toBeNull()
    expect(source.status).toHaveBeenCalledTimes(1)
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})
it("refuses foreground takeover during an admitted normal unlock submission", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const { source, lock } = fixture()
  lock()
  let resolve!: (state: VaultState) => void
  source.unlock = vi.fn(
    () =>
      new Promise<VaultState>((done) => {
        resolve = done
      }),
  )
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const begun = vi.fn()
  const result = vi.fn()
  try {
    await act(async () =>
      root.render(
        <VaultProvider source={source}>
          <ForegroundProbe result={begun} />
          <ProtectedUse result={result} />
        </VaultProvider>,
      ),
    )
    await act(async () => button("Protected use").click())
    await act(async () => button("Submit normal unlock").click())
    await act(async () => button("Begin foreground").click())
    expect(begun).toHaveBeenLastCalledWith(false)
    expect(
      document.querySelector('[data-vault-dialog="unlock"][data-busy]'),
    ).not.toBeNull()
    expect(result).not.toHaveBeenCalled()
    await act(async () => resolve("unlocked"))
    expect(result).toHaveBeenLastCalledWith(true)
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})

it("releases a pending change's UI ownership on unmount and ignores its late completion", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const { source, lock } = fixture()
  let resolve!: (state: VaultState) => void
  source.changePassphrase = vi.fn(
    () =>
      new Promise<VaultState>((done) => {
        resolve = done
      }),
  )
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const result = vi.fn()
  const view = (show: boolean) => (
    <VaultProvider source={source}>
      {show ? <VaultPassphraseControl /> : null}
      <ProtectedUse result={result} />
    </VaultProvider>
  )
  try {
    await act(async () => root.render(view(true)))
    await act(async () => button("Change passphrase").click())
    await act(async () => button("Submit change").click())
    lock()
    await act(async () => root.render(view(false)))
    await act(async () => button("Protected use").click())
    expect(document.querySelector('[data-vault-dialog="unlock"]')).not.toBeNull()
    await act(async () => resolve("locked"))
    expect(document.querySelector("[data-vault-change]")).toBeNull()
    expect(document.querySelector('[data-vault-dialog="unlock"]')).not.toBeNull()
    expect(source.changePassphrase).toHaveBeenCalledTimes(1)
    expect(source.status).toHaveBeenCalledTimes(2)
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})
it("a stale requester release cannot remove a newer foreground owner", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const { source, lock } = fixture()
  lock()
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const begun = vi.fn()
  const result = vi.fn()
  try {
    await act(async () =>
      root.render(
        <VaultProvider source={source}>
          <ForegroundProbe result={begun} label="first" />
          <ForegroundProbe result={begun} label="second" />
          <ProtectedUse result={result} />
        </VaultProvider>,
      ),
    )
    await act(async () => button("Begin first").click())
    expect(begun).toHaveBeenLastCalledWith(true)
    await act(async () => button("End first").click())
    await act(async () => button("Begin second").click())
    expect(begun).toHaveBeenLastCalledWith(true)
    await act(async () => button("End first").click())
    await act(async () => button("Protected use").click())
    expect(result).toHaveBeenLastCalledWith(false)
    expect(document.querySelector("[data-vault-dialog]")).toBeNull()
    expect(source.status).not.toHaveBeenCalled()
    await act(async () => button("End second").click())
    await act(async () => button("Protected use").click())
    expect(document.querySelector('[data-vault-dialog="unlock"]')).not.toBeNull()
  } finally {
    await act(async () => root.unmount())
    host.remove()
  }
})
