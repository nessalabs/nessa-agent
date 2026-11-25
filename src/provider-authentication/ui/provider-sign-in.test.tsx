// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { ChatgptSignInCard, ProviderSignInCard } from "./provider-sign-in"

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

it("says when sign-in could not open, and offers it again", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const container = document.createElement("div")
  const root = createRoot(container)
  const button = () => container.querySelector("button")
  try {
    await React.act(async () =>
      root.render(
        <ProviderSignInCard
          provider="claude"
          onSignIn={async () => {
            throw new Error("no browser")
          }}
          canSignIn={async () => true}
        />,
      ),
    )
    await React.act(async () => button()?.click())
    expect(container.textContent).toContain("Could not open sign-in. Try again.")
    expect(button()?.disabled).toBe(false)
  } finally {
    await React.act(async () => root.unmount())
  }
})

it("signs Codex in with ChatGPT, linking to the browser while it waits", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const container = document.createElement("div")
  const root = createRoot(container)
  let finish: (end: "signed_in" | "timed_out") => void = () => {}
  const signIn = {
    available: async () => true,
    signIn: vi.fn(
      (opened: (url: string) => void) =>
        new Promise<"signed_in" | "timed_out">((resolve) => {
          opened("https://auth.example/a")
          finish = resolve
        }),
    ),
  }
  const button = () => {
    const found = container.querySelector<HTMLButtonElement>(
      "[data-chatgpt-card-continue]",
    )
    if (!found) throw new Error("Missing ChatGPT button")
    return found
  }
  try {
    await React.act(async () => root.render(<ChatgptSignInCard signIn={signIn} />))
    expect(button().disabled).toBe(false)
    await React.act(async () => button().click())
    expect(button().disabled).toBe(true)
    expect(
      container.querySelector("[data-chatgpt-card-open]")?.getAttribute("href"),
    ).toBe("https://auth.example/a")
    await React.act(async () => finish("timed_out"))
    expect(container.textContent).toContain("The sign-in took too long. Try again.")
    await React.act(async () => button().click())
    await React.act(async () => finish("signed_in"))
    expect(container.textContent).toContain(
      "Signed in with ChatGPT. Send your message again.",
    )
    expect(container.querySelector("[data-chatgpt-card-continue]")).toBeNull()

    await React.act(async () =>
      root.render(
        <ChatgptSignInCard
          key="unavailable"
          signIn={{ ...signIn, available: async () => false }}
        />,
      ),
    )
    expect(button().disabled).toBe(true)
  } finally {
    await React.act(async () => root.unmount())
  }
})
