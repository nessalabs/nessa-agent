// @vitest-environment jsdom
/**
 * Settings › Accounts › GitHub: the row rests without a gateway; with one it
 * offers Connect, shows the code while the gateway waits and reads again
 * only then, shows the signed-in account with its picture and "Connected"
 * (its actions out only under the pointer or focus), and says why a try
 * ended.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { GithubGateway } from "../adapters/github-gateway"
import type { GithubOutcome, GithubView } from "../model/github"
import { GithubProvider, GithubRow, type GithubAvatar } from "./github-row"

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
  vi.useRealTimers()
})

const installUrl = "https://github.com/apps/nessa-agent/installations/new"
const signedOut: GithubView = { kind: "signedOut", installUrl }
const connecting: GithubView = {
  kind: "connecting",
  userCode: "WDJB-MJHT",
  verificationUri: "https://github.com/login/device",
  expiresInSeconds: 900,
  installUrl,
}
const connected: GithubView = {
  kind: "connected",
  accountId: 7,
  login: "octo",
  installUrl,
}

const row = () => host.querySelector('[data-setting="github"]')
const state = () => row()?.getAttribute("data-github-state")
const button = (name: string) =>
  [...(row()?.querySelectorAll("button") ?? [])].find(
    (each) => (each.getAttribute("aria-label") ?? each.textContent) === name,
  )

/** A gateway whose state is `view`, changed by its actions as the gateway would. */
function fake(initial: GithubView) {
  let view = initial
  const calls: string[] = []
  const answer = (): Promise<GithubOutcome> => Promise.resolve({ ok: true, view })
  const gateway: GithubGateway = {
    pollMs: 2_000,
    liveEvents: (enabled) => {
      calls.push(`live:${enabled}`)
      view = {
        kind: "connected",
        accountId: 7,
        login: "octo",
        installUrl,
        liveEvents: { enabled, repositories: enabled ? 1 : 0, gaps: 0 },
      }
      return answer()
    },
    read: () => (calls.push("read"), answer()),
    connect: () => {
      calls.push("connect")
      view = connecting
      return answer()
    },
    cancel: () => {
      calls.push("cancel")
      view = { kind: "signedOut", installUrl, ended: "The sign-in was cancelled." }
      return answer()
    },
    disconnect: () => {
      calls.push("disconnect")
      view = signedOut
      return answer()
    },
  }
  return { gateway, calls, set: (next: GithubView) => (view = next) }
}

async function show(gateway: GithubGateway | undefined, avatar?: GithubAvatar) {
  await act(async () =>
    root.render(
      <GithubProvider gateway={gateway} avatar={avatar}>
        <GithubRow />
      </GithubProvider>,
    ),
  )
  await act(async () => {})
}

it("rests when this window has no gateway", async () => {
  await show(undefined)
  expect(row()?.textContent).toContain("Not available in this window.")
  expect(row()?.getAttribute("data-disabled")).not.toBeNull()
  expect(row()?.querySelector("button")).toBeNull()
})

it("connects, shows the code and where to enter it, and names the account once entered", async () => {
  vi.useFakeTimers()
  const github = fake(signedOut)
  await show(github.gateway)
  expect(state()).toBe("signed-out")
  await act(async () => button("Connect…")?.click())
  expect(state()).toBe("connecting")
  expect(
    row()?.querySelector("[data-github-code]")?.getAttribute("data-github-code"),
  ).toBe("WDJB-MJHT")
  expect(row()?.querySelector("[data-github-open]")?.getAttribute("href")).toBe(
    "https://github.com/login/device",
  )
  github.set(connected)
  await act(async () => vi.advanceTimersByTime(2_000))
  expect(state()).toBe("connected")
  expect(row()?.querySelector('[data-slot="settings-row-detail"]')?.textContent).toMatch(
    /^@octo /,
  )
  expect(row()?.querySelector(".settings-github-status")?.textContent).toBe("Connected")
  expect(row()?.querySelector("[data-github-repositories]")?.getAttribute("href")).toBe(
    installUrl,
  )
  // Signed in, nothing waits on GitHub: the row stops reading on a timer.
  const reads = github.calls.filter((call) => call === "read").length
  await act(async () => vi.advanceTimersByTime(10_000))
  expect(github.calls.filter((call) => call === "read").length).toBe(reads)
})

it("cancels a waiting code and says why the try ended", async () => {
  const github = fake(connecting)
  await show(github.gateway)
  await act(async () => button("Cancel")?.click())
  expect(state()).toBe("signed-out")
  expect(row()?.textContent).toContain("The sign-in was cancelled.")
})

it("disconnects, and says that this does not revoke the app on GitHub", async () => {
  const github = fake(connected)
  await show(github.gateway)
  expect(host.querySelector('[popover="auto"]')?.textContent).toContain(
    "To revoke GitHub access",
  )
  await act(async () => button("Disconnect")?.click())
  expect(github.calls).toContain("disconnect")
  expect(state()).toBe("signed-out")
})

it("draws the account's picture through the composed avatar, and only an initial without one", async () => {
  const asked: { id: number; login: string }[] = []
  const avatar: GithubAvatar = (account) => {
    asked.push(account)
    return <span data-fake-avatar={account.login} />
  }
  await show(fake(connected).gateway, avatar)
  expect(asked.at(-1)).toEqual({ id: 7, login: "octo" })
  expect(row()?.querySelector('[data-fake-avatar="octo"]')).not.toBeNull()
  await show(fake(connected).gateway)
  expect(row()?.querySelector('[data-slot="contact-avatar"]')?.textContent).toBe("O")
  expect(row()?.querySelector("img")).toBeNull()
})

it("keeps its actions in reach: one Tab away at rest, out while one is in flight or failed", async () => {
  const github = fake(connected)
  let answer: ((outcome: GithubOutcome) => void) | undefined
  const gateway: GithubGateway = {
    ...github.gateway,
    disconnect: () => new Promise<GithubOutcome>((resolve) => (answer = resolve)),
  }
  await show(gateway)
  // At rest the actions wait for the pointer or focus (settings.css), and are
  // in the tab order, so focus is what brings them out.
  expect(row()?.hasAttribute("data-github-settled")).toBe(true)
  const link = row()?.querySelector<HTMLAnchorElement>("[data-github-repositories]")
  expect(link?.tabIndex).toBe(0)
  expect(link?.getAttribute("aria-label")).toBe("Choose repositories")
  expect(button("Disconnect")?.disabled).toBe(false)
  await act(async () => button("Disconnect")?.click())
  expect(row()?.hasAttribute("data-github-settled")).toBe(false)
  await act(async () =>
    answer?.({ ok: false, sentence: "The gateway is busy. Try again." }),
  )
  expect(state()).toBe("connected")
  expect(row()?.hasAttribute("data-github-settled")).toBe(false)
})

it("shows a refused action's sentence over what the gateway now says", async () => {
  const github = fake(connected)
  const gateway: GithubGateway = {
    ...github.gateway,
    disconnect: () =>
      Promise.resolve({
        ok: false,
        sentence: "Couldn't remove the sign-in. It is still in use; try again.",
      }),
  }
  await show(gateway)
  await act(async () => button("Disconnect")?.click())
  expect(state()).toBe("connected")
  expect(row()?.textContent).toContain("Couldn't remove the sign-in.")
})

it("says a first read that failed, and reads again on Try again", async () => {
  let fails = true
  const github = fake(signedOut)
  const gateway: GithubGateway = {
    ...github.gateway,
    read: () =>
      fails
        ? Promise.resolve({
            ok: false,
            sentence: "The gateway didn't answer. Try again.",
          })
        : github.gateway.read(),
  }
  await show(gateway)
  expect(state()).toBe("unread")
  expect(row()?.textContent).toContain("The gateway didn't answer.")
  fails = false
  await act(async () => button("Try again")?.click())
  expect(state()).toBe("signed-out")
})

it("keeps the code shown, and polling, when one read fails while it waits", async () => {
  vi.useFakeTimers()
  let fails = false
  const github = fake(connecting)
  const gateway: GithubGateway = {
    ...github.gateway,
    read: () =>
      fails
        ? Promise.resolve({ ok: false, sentence: "The gateway is busy. Try again." })
        : github.gateway.read(),
  }
  await show(gateway)
  fails = true
  await act(async () => vi.advanceTimersByTime(2_000))
  expect(state()).toBe("connecting")
  expect(row()?.querySelector("[data-github-code]")).not.toBeNull()
  expect(row()?.textContent).toContain("The gateway is busy.")
  // The next read answers the same state: the failure is no longer said.
  fails = false
  await act(async () => vi.advanceTimersByTime(2_000))
  expect(state()).toBe("connecting")
  expect(row()?.textContent).not.toContain("The gateway is busy.")
  expect(row()?.textContent).toContain("Enter this code on GitHub to sign in.")
  github.set(connected)
  await act(async () => vi.advanceTimersByTime(2_000))
  expect(state()).toBe("connected")
  expect(row()?.textContent).not.toContain("The gateway is busy.")
})

it("lets a read slower than the poll finish, rather than overtaking it", async () => {
  vi.useFakeTimers()
  const github = fake(connecting)
  let answer: ((outcome: GithubOutcome) => void) | undefined
  let slow = false
  let reads = 0
  const gateway: GithubGateway = {
    ...github.gateway,
    read: () => {
      reads += 1
      if (!slow) return github.gateway.read()
      return new Promise<GithubOutcome>((resolve) => (answer = resolve))
    },
  }
  await show(gateway)
  slow = true
  const before = reads
  await act(async () => vi.advanceTimersByTime(2_000))
  expect(reads).toBe(before + 1)
  // Two more polls pass while that read is out: neither starts another.
  await act(async () => vi.advanceTimersByTime(4_000))
  expect(reads).toBe(before + 1)
  await act(async () => answer?.({ ok: true, view: connected }))
  expect(state()).toBe("connected")
})

it("opts in through the gateway and clears live controls on disconnect", async () => {
  const github = fake({
    ...connected,
    liveEvents: { enabled: false, repositories: 0, gaps: 0 },
  })
  await show(github.gateway)
  const control = () => {
    const value = host.querySelector<HTMLButtonElement>('[role="switch"]')
    if (!value) throw new Error("live events switch absent")
    return value
  }
  expect(control().getAttribute("aria-checked")).toBe("false")
  const live = host.querySelector("[data-github-live]")
  expect(live).not.toBeNull()
  expect(row()?.contains(live)).toBe(false)
  expect(live?.querySelector('[data-slot="settings-row-detail"]')?.textContent).toBe(
    "PR, issue and CI updates.",
  )
  await act(async () => control().click())
  expect(github.calls).toContain("live:true")
  expect(control().getAttribute("aria-checked")).toBe("true")
  await act(async () => button("Disconnect")?.click())
  expect(host.querySelector('[role="switch"]')).toBeNull()
})
it("does not optimistically enable live events when the gateway refuses", async () => {
  const github = fake({
    ...connected,
    liveEvents: { enabled: false, repositories: 0, gaps: 0 },
  })
  await show({
    ...github.gateway,
    liveEvents: () =>
      Promise.resolve({ ok: false, sentence: "Repository permissions are missing." }),
  })
  await act(async () => host.querySelector<HTMLButtonElement>('[role="switch"]')?.click())
  expect(host.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe(
    "false",
  )
  expect(host.textContent).toContain("Repository permissions are missing.")
})
