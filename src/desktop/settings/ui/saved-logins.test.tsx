// @vitest-environment jsdom
/**
 * Saved logins: Settings lists one row per agent, saves a pasted login and
 * clears the field, says only "Saved" afterwards with Remove, and says why a
 * change was refused; a paired computer's switch in Devices sends its
 * environment and reads back what the gateway answered.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { LoginAgent } from "@nessa/client"
import type {
  SavedLogins,
  SavedLoginsGateway,
  SavedLoginsOutcome,
} from "../adapters/saved-logins-gateway"
import { VaultProvider, useVaultUnlock, type VaultSource } from "../../../vault"
import type { VoiceKeyGateway } from "../../voice/adapters/voice-gateway"
import { SavedLoginsGroup, SavedLoginsProvider } from "./saved-logins"
import { VoiceKeyProvider, VoiceKeyRow } from "./voice-key-row"

let root: Root
let host: HTMLDivElement

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

function fake({ chatgpt = false }: { chatgpt?: boolean } = {}) {
  let logins: SavedLogins = {
    saved: { claude: false, codex: false, opencode: false },
    ...(chatgpt ? { chatgpt: { accounts: [], signIn: { kind: "idle" as const } } } : {}),
  }
  const accounts = () => logins.chatgpt?.accounts ?? []
  const withChatgpt = (next: NonNullable<SavedLogins["chatgpt"]>) => {
    logins = { ...logins, chatgpt: next }
  }
  const calls: unknown[][] = []
  let refuse: string | undefined
  const answer = (): Promise<SavedLoginsOutcome> =>
    Promise.resolve(
      refuse ? { kind: "refused", sentence: refuse } : { kind: "read", logins },
    )
  const gateway: SavedLoginsGateway = {
    read: answer,
    save: (agent: LoginAgent, login: string) => {
      calls.push(["save", agent, login])
      if (!refuse) logins = { ...logins, saved: { ...logins.saved, [agent]: true } }
      return answer()
    },
    remove: (agent: LoginAgent) => {
      calls.push(["remove", agent])
      logins = { ...logins, saved: { ...logins.saved, [agent]: false } }
      return answer()
    },
    chatgptSignIn: (accountId?: string) => {
      calls.push(["chatgptSignIn", accountId])
      withChatgpt({
        accounts: accounts(),
        signIn: {
          kind: "waiting",
          authorizeUrl: "https://auth.example/authorize?state=s",
          ...(accountId ? { accountId } : {}),
        },
      })
      return answer()
    },
    chatgptCancel: () => {
      calls.push(["chatgptCancel"])
      withChatgpt({ accounts: accounts(), signIn: { kind: "idle", last: "cancelled" } })
      return answer()
    },
    chatgptUse: (accountId: string | null) => {
      calls.push(["chatgptUse", accountId])
      withChatgpt({
        accounts: accounts().map((each) => ({ ...each, inUse: each.id === accountId })),
        signIn: { kind: "idle" },
      })
      return answer()
    },
    chatgptRemove: (accountId: string) => {
      calls.push(["chatgptRemove", accountId])
      withChatgpt({
        accounts: accounts().filter((each) => each.id !== accountId),
        signIn: { kind: "idle" },
      })
      return answer()
    },
  }
  /** The browser came back: the account is kept, in use when it is the first. */
  const signedIn = (id: string, email: string) =>
    withChatgpt({
      accounts: [
        ...accounts(),
        { id, email, inUse: accounts().length === 0, needsSignIn: false },
      ],
      signIn: { kind: "idle", last: "signed_in" },
    })
  return {
    gateway,
    calls,
    signedIn,
    refuseWith: (sentence: string) => (refuse = sentence),
  }
}

function need<T>(value: T | null | undefined): T {
  if (value === null || value === undefined) throw new Error("not on the page")
  return value
}

async function type(input: HTMLInputElement, value: string) {
  const setter = need(
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set,
  )
  await act(async () => {
    setter.call(input, value)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}

const input = (agent: string) =>
  need(host.querySelector<HTMLInputElement>(`[data-saved-login-input="${agent}"]`))
const save = (agent: string) =>
  need(host.querySelector<HTMLButtonElement>(`[data-saved-login-save="${agent}"]`))
const row = (agent: string) => need(host.querySelector(`[data-saved-login="${agent}"]`))

it("saves a pasted login, then shows it only as saved, and removes it", async () => {
  const { gateway, calls } = fake()
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={gateway}>
        <SavedLoginsGroup />
      </SavedLoginsProvider>,
    ),
  )
  expect(host.querySelectorAll("[data-saved-login]").length).toBe(3)
  await type(input("codex"), "  sk-codex  ")
  await act(async () => save("codex").click())
  expect(calls).toEqual([["save", "codex", "sk-codex"]])
  expect(input("codex").value).toBe("")
  expect(row("codex").querySelector('[data-saved-login-state="saved"]')).not.toBeNull()
  expect(host.textContent).not.toContain("sk-codex")
  expect(save("codex").textContent).toBe("Replace")
  const remove = need(
    [...row("codex").querySelectorAll("button")].find(
      (button) => button.textContent === "Remove",
    ),
  )
  await act(async () => remove.click())
  expect(calls.at(-1)).toEqual(["remove", "codex"])
  expect(row("codex").querySelector('[data-saved-login-state="saved"]')).toBeNull()
})

it("keeps what was typed and says why when the gateway refuses", async () => {
  const { gateway, refuseWith } = fake()
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={gateway}>
        <SavedLoginsGroup />
      </SavedLoginsProvider>,
    ),
  )
  refuseWith("Unlock Nessa to change saved logins.")
  await type(input("claude"), "sk-ant-oat")
  await act(async () => save("claude").click())
  expect(input("claude").value).toBe("sk-ant-oat")
  expect(host.querySelector('[role="alert"]')?.textContent).toBe(
    "Unlock Nessa to change saved logins.",
  )
})

it("rests without a gateway", async () => {
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={undefined}>
        <SavedLoginsGroup />
      </SavedLoginsProvider>,
    ),
  )
  expect(input("claude").disabled).toBe(true)
})

const button = (selector: string) => need(host.querySelector<HTMLButtonElement>(selector))

it("signs a ChatGPT account in from Codex's row, and reads until it ends", async () => {
  const { gateway, calls, signedIn } = fake({ chatgpt: true })
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={gateway}>
        <SavedLoginsGroup pollMs={5} />
      </SavedLoginsProvider>,
    ),
  )
  // Only Codex's row offers it.
  expect(host.querySelectorAll("[data-chatgpt-continue]").length).toBe(1)
  expect(row("codex").querySelector("[data-chatgpt-continue]")?.textContent).toBe(
    "Continue with ChatGPT",
  )
  await act(async () => button("[data-chatgpt-continue]").click())
  expect(calls).toEqual([["chatgptSignIn", undefined]])
  expect(host.querySelector("[data-chatgpt-open]")?.getAttribute("href")).toBe(
    "https://auth.example/authorize?state=s",
  )
  signedIn("0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e", "me@example.com")
  await act(async () => new Promise((resolve) => setTimeout(resolve, 30)))
  expect(host.querySelector("[data-chatgpt-open]")).toBeNull()
  expect(host.querySelector('[data-chatgpt-account="me@example.com"]')).not.toBeNull()
  expect(host.querySelector("[data-chatgpt-in-use]")?.textContent).toContain("In use")
  expect(host.querySelector('[data-chatgpt-ended="signed_in"]')?.textContent).toBe(
    "Signed in with ChatGPT.",
  )
  expect(button("[data-chatgpt-continue]").textContent).toBe(
    "Add another ChatGPT account",
  )
})

it("chooses, sets aside and removes ChatGPT accounts, and cancels a sign-in quietly", async () => {
  const { gateway, calls, signedIn } = fake({ chatgpt: true })
  const first = "0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e"
  const second = "1b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e"
  signedIn(first, "one@example.com")
  signedIn(second, "two@example.com")
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={gateway}>
        <SavedLoginsGroup />
      </SavedLoginsProvider>,
    ),
  )
  await act(async () => button(`[data-chatgpt-use="${second}"]`).click())
  expect(calls.at(-1)).toEqual(["chatgptUse", second])
  expect(
    host.querySelector("[data-chatgpt-in-use]")?.getAttribute("data-chatgpt-account"),
  ).toBe("two@example.com")
  await act(async () => button(`[data-chatgpt-stop-using="${second}"]`).click())
  expect(calls.at(-1)).toEqual(["chatgptUse", null])
  expect(host.textContent).toContain("Codex uses the key above.")
  await act(async () => button(`[data-chatgpt-remove="${first}"]`).click())
  expect(calls.at(-1)).toEqual(["chatgptRemove", first])
  expect(host.querySelectorAll("[data-chatgpt-account]").length).toBe(1)
  await act(async () => button("[data-chatgpt-continue]").click())
  await act(async () => button("[data-chatgpt-cancel]").click())
  expect(calls.at(-1)).toEqual(["chatgptCancel"])
  expect(host.querySelector("[data-chatgpt-ended]")).toBeNull()
})

it("shows no ChatGPT accounts where the gateway keeps none", async () => {
  const { gateway } = fake()
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={gateway}>
        <SavedLoginsGroup />
      </SavedLoginsProvider>,
    ),
  )
  expect(host.querySelector("[data-chatgpt-accounts]")).toBeNull()
})

it("re-reads saved logins and voice key once the vault unlocks", async () => {
  let state: "locked" | "unlocked" = "locked"
  const { gateway, refuseWith } = fake({ chatgpt: true })
  refuseWith("Unlock Nessa to change saved logins.")
  const voice: VoiceKeyGateway = {
    read: async () =>
      state === "locked"
        ? { kind: "refused", sentence: "Nessa is locked." }
        : { kind: "read", keySaved: false },
    save: async () => ({ kind: "read", keySaved: true }),
    forget: async () => ({ kind: "read", keySaved: false }),
  }
  const source: VaultSource = {
    status: async () => state,
    setup: async () => {
      state = "unlocked"
      return state
    },
    unlock: async () => {
      state = "unlocked"
      refuseWith("")
      return state
    },
    changePassphrase: async () => state,
    lock: async () => {
      state = "locked"
      return state
    },
  }
  function UnlockTrigger() {
    const ensure = useVaultUnlock()
    return (
      <button type="button" data-test-unlock onClick={() => void ensure()}>
        Unlock
      </button>
    )
  }
  await act(async () =>
    root.render(
      <VaultProvider source={source}>
        <UnlockTrigger />
        <VoiceKeyProvider gateway={voice}>
          <VoiceKeyRow />
        </VoiceKeyProvider>
        <SavedLoginsProvider gateway={gateway}>
          <SavedLoginsGroup />
        </SavedLoginsProvider>
      </VaultProvider>,
    ),
  )
  expect(host.querySelector("[data-chatgpt-continue]")).toBeNull()
  expect(host.textContent).toContain("Nessa is locked.")

  await act(async () => button("[data-test-unlock]").click())
  const input = document.querySelector<HTMLInputElement>(
    '[data-vault-dialog="unlock"] input[type="password"]',
  )
  if (!input) throw new Error("expected unlock passphrase input")
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set
    set?.call(input, "test-passphrase-123")
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
  const form = document.querySelector<HTMLFormElement>(
    '[data-vault-dialog="unlock"] form',
  )
  if (!form) throw new Error("expected unlock form")
  await act(async () => {
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
  })
  expect(document.querySelector('[data-vault-dialog="unlock"]')).toBeNull()
  expect(host.querySelector("[data-chatgpt-continue]")?.textContent).toBe(
    "Continue with ChatGPT",
  )
  expect(host.querySelector(".settings-saved-logins-error")).toBeNull()
  expect(host.textContent).toContain("Not connected")
  expect(host.textContent).not.toContain("Nessa is locked.")
})

it("applies a slow ChatGPT polling response even when read latency exceeds pollMs", async () => {
  const { gateway, signedIn } = fake({ chatgpt: true })
  let reads = 0
  const slowGateway: SavedLoginsGateway = {
    ...gateway,
    read: () => {
      reads += 1
      if (reads === 1) return gateway.read()
      return new Promise((resolve) => {
        setTimeout(() => void gateway.read().then(resolve), 40)
      })
    },
  }
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={slowGateway}>
        <SavedLoginsGroup pollMs={10} />
      </SavedLoginsProvider>,
    ),
  )
  await act(async () => button("[data-chatgpt-continue]").click())
  expect(host.querySelector("[data-chatgpt-open]")).not.toBeNull()
  signedIn("2b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e", "slow@example.com")
  await act(async () => new Promise((resolve) => setTimeout(resolve, 90)))
  expect(host.querySelector("[data-chatgpt-open]")).toBeNull()
  expect(host.querySelector('[data-chatgpt-account="slow@example.com"]')).not.toBeNull()
})
