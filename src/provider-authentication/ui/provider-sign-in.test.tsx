// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { ProviderSignInCard } from "./provider-sign-in"

it("keeps unsupported and superseded capability checks from enabling login", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const container = document.createElement("div")
  const root = createRoot(container)
  const onSignIn = vi.fn(async () => {})
  let confirmFirst: (value: boolean) => void = () => {}
  const first = () =>
    new Promise<boolean>((resolve) => {
      confirmFirst = resolve
    })
  let confirmNext: (value: boolean) => void = () => {}
  const next = () =>
    new Promise<boolean>((resolve) => {
      confirmNext = resolve
    })
  const button = () => {
    const found = container.querySelector("button")
    if (!found) throw new Error("Missing provider login button")
    return found
  }
  try {
    await React.act(async () =>
      root.render(
        <ProviderSignInCard provider="claude" onSignIn={onSignIn} canSignIn={first} />,
      ),
    )
    expect(button().disabled).toBe(true)
    await React.act(async () =>
      root.render(
        <ProviderSignInCard provider="claude" onSignIn={onSignIn} canSignIn={next} />,
      ),
    )
    await React.act(async () => confirmFirst(true))
    expect(button().disabled).toBe(true)
    await React.act(async () => confirmNext(false))
    expect(button().disabled).toBe(true)
    await React.act(async () => button().click())
    expect(onSignIn).not.toHaveBeenCalled()

    const supported = async () => true
    await React.act(async () =>
      root.render(
        <ProviderSignInCard provider="codex" onSignIn={onSignIn} canSignIn={supported} />,
      ),
    )
    expect(button().disabled).toBe(false)
    const unavailable = async () => {
      throw new Error("host unavailable")
    }
    await React.act(async () =>
      root.render(
        <ProviderSignInCard
          provider="codex"
          onSignIn={onSignIn}
          canSignIn={unavailable}
        />,
      ),
    )
    expect(button().disabled).toBe(true)
    await React.act(async () => button().click())
    expect(onSignIn).not.toHaveBeenCalled()
  } finally {
    await React.act(async () => root.unmount())
  }
})
