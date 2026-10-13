// @vitest-environment jsdom
/**
 * The share dialog as drawn: the grants and what each holds, a share and a
 * revoke sent once each and the list read again, every refusal in words and
 * never a wire code, the hosts held to the room the patterns leave, and the
 * notes on what a policy does not stop.
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { makeDesktopStore } from "../../../store"
import type { SharingSource } from "../../application/ports"
import { inMemorySharing } from "../../adapters/in-memory/in-memory-sharing"
import { closeShare, openShare, readShares } from "../../adapters/store/commands"
import { fakeSource, keptFilter, roomyGrid, settle } from "../../testing"
import { driveNotes } from "./share-copy"
import { ShareDialogHost } from "./share-dialog"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  if (typeof window.matchMedia !== "function")
    window.matchMedia = ((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as typeof window.matchMedia
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

async function render(sharing: SharingSource) {
  let next = 0
  const store = makeDesktopStore({
    workspace: fakeSource(),
    now: () => 1000,
    newId: () => `id-${++next}`,
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    sharing,
  })
  await act(async () =>
    root.render(
      <StrictMode>
        <Provider store={store}>
          <ShareDialogHost />
        </Provider>
      </StrictMode>,
    ),
  )
  return store
}

async function open(store: Awaited<ReturnType<typeof render>>, sessionId = "a") {
  await act(async () => {
    await store.dispatch(openShare({ sessionId }))
  })
}

const dialog = () => document.querySelector<HTMLElement>("[data-share-dialog]")
const find = <T extends HTMLElement = HTMLElement>(selector: string) => {
  const found = dialog()?.querySelector<T>(selector)
  if (!found) throw new Error(`nothing matches ${selector}`)
  return found
}
const click = async (element: HTMLElement) => {
  await act(async () => {
    element.click()
    await settle()
  })
}
/** Chooses a kit radio card or option by its visible label. */
const choose = async (group: string, label: string) => {
  const option = [...find(group).querySelectorAll<HTMLElement>("label, button")].find(
    (each) => each.textContent?.includes(label),
  )
  if (!option) throw new Error(`no ${label} in ${group}`)
  await click(option)
}
const type = async (input: HTMLInputElement, value: string) => {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(
      input,
      value,
    )
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}
const submit = () => find<HTMLButtonElement>('[data-share-action="submit"]')
const hostBoxes = () => [
  ...find("[data-share-hosts]").querySelectorAll<HTMLInputElement>(
    'input[type="checkbox"]',
  ),
]
const box = (index: number) => {
  const found = hostBoxes()[index]
  if (!found) throw new Error(`no host box ${index}`)
  return found
}
const within = (scope: HTMLElement, selector: string) => {
  const found = scope.querySelector<HTMLElement>(selector)
  if (!found) throw new Error(`nothing matches ${selector}`)
  return found
}
const wireCodes =
  /share_target_not_paired|invalid_request|drive_policy_unenforceable|not-paired|unanswered/

describe("the share dialog", () => {
  it("draws nothing until opened, and goes when closed", async () => {
    const store = await render(inMemorySharing())
    expect(dialog()).toBeNull()
    await open(store)
    expect(dialog()?.getAttribute("data-share-state")).toBe("read")
    expect(dialog()?.textContent).toContain("Not shared with any device yet.")
    await act(async () => {
      store.dispatch(closeShare())
    })
    expect(dialog()).toBeNull()
  })

  it("shares Read with a chosen device, lists it, and revokes it", async () => {
    const sharing = inMemorySharing({ now: () => 1_700_000_000_000 })
    const store = await render(sharing)
    await open(store)
    expect(submit().disabled).toBe(true)
    await choose("[data-share-targets]", "-phone")
    expect(submit().textContent).toBe("Share")
    await click(submit())
    expect(sharing.calls.filter((call) => call.method === "share")).toHaveLength(1)
    expect(find("[data-share-notice]").textContent).toBe("Shared.")
    const row = find('[data-share-grant="sample-phone"]')
    expect(row.textContent).toContain("Read")
    // The device holds a grant now: the form changes it rather than adding one.
    expect(submit().textContent).toBe("Update")
    // The form now builds on what the device holds, so it is not held as
    // changed under it.
    expect(dialog()?.querySelector("[data-share-reason]")).toBeNull()
    expect(submit().disabled).toBe(false)
    await click(within(row, '[data-share-action="revoke"]'))
    // Asked first; nothing is sent until it is confirmed.
    expect(row.textContent).toContain("Revoke this device’s access")
    expect(sharing.calls.map((call) => call.method)).toEqual(["read", "share", "read"])
    await click(within(row, '[data-share-action="confirm-revoke"]'))
    expect(sharing.calls.map((call) => call.method)).toEqual([
      "read",
      "share",
      "read",
      "unshare",
      "read",
    ])
    expect(find("[data-share-notice]").textContent).toBe("No longer shared.")
    expect(dialog()?.querySelector("[data-share-grant]")).toBeNull()
  })

  it("says a refusal in words, never its code", async () => {
    const store = await render(
      inMemorySharing({
        refuse: (method) => (method === "share" ? "not-paired" : undefined),
      }),
    )
    await open(store)
    await choose("[data-share-targets]", "-phone")
    await click(submit())
    const notice = find("[data-share-notice]").textContent ?? ""
    expect(notice).toContain("no longer linked to you")
    expect(dialog()?.textContent).not.toMatch(wireCodes)
  })

  it("says a failed read with Try Again, which reads again", async () => {
    let refuse = true
    const store = await render(
      inMemorySharing({ refuse: () => (refuse ? "unanswered" : undefined) }),
    )
    await open(store)
    expect(find("[data-share-read-failure]").textContent).toContain(
      "No answer this window could read came from the gateway.",
    )
    expect(dialog()?.textContent).not.toMatch(wireCodes)
    refuse = false
    await click(find('[data-share-action="retry"]'))
    expect(dialog()?.querySelector("[data-share-read-failure]")).toBeNull()
  })

  it("asks Drive for a tool policy and hosts, and says what a policy cannot stop", async () => {
    const sharing = inMemorySharing()
    const share = sharing.share
    const sent: unknown[] = []
    sharing.share = async (sessionId, request, requestId) => {
      sent.push(request)
      return share(sessionId, request, requestId)
    }
    const store = await render(sharing)
    await open(store)
    await choose("[data-share-targets]", "-phone")
    expect(dialog()?.querySelector("[data-share-drive]")).toBeNull()
    await choose("[data-share-role]", "Drive")
    const notes = find("[data-share-notes]").textContent ?? ""
    expect(notes).toContain(driveNotes.unasked)
    expect(notes).toContain(driveNotes.hosts)
    expect(notes).toContain(driveNotes.enforceable)
    expect(hostBoxes()).toHaveLength(2)
    await click(box(1))
    await choose("[data-share-presets]", "No shell")
    await click(submit())
    expect(sent).toEqual([
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: {
          kind: "preset",
          preset: "no_shell",
          commandHosts: ["me@build.example"],
        },
      },
    ])
    expect(find('[data-share-grant="sample-phone"]').textContent).toContain(
      "Drive · No shell, commands on me@build.example",
    )
  })

  it("holds the host list to the room the patterns leave", async () => {
    const limits = { patterns: 16, hosts: 3, hostBytes: 8, entries: 4 }
    const memory = inMemorySharing({ commandHosts: ["a", "b", "c", "much-too-long"] })
    const sharing: SharingSource = {
      ...memory,
      read: async (sessionId) => ({ ...(await memory.read(sessionId)), limits }),
    }
    const store = await render(sharing)
    await open(store)
    await choose("[data-share-targets]", "-phone")
    await choose("[data-share-role]", "Drive")
    const tooLong = box(3)
    expect(tooLong.closest("[data-slot=checkbox-list-row]")?.textContent).toContain(
      "Name too long for a grant",
    )
    for (const box of hostBoxes().slice(0, 3)) await click(box)
    expect(hostBoxes().filter((box) => box.checked)).toHaveLength(3)
    await choose("[data-share-presets]", "Custom")
    expect(find("[data-share-reason]").textContent).toContain("Name at least one tool")
    // Two patterns leave room for two hosts of the four entries.
    await type(find<HTMLInputElement>('[data-share-pattern="allow"]'), "Read, Grep")
    expect(find("[data-share-hosts]").textContent).toContain("3 selected, room for 2")
    expect(find("[data-share-reason]").textContent).toContain("Too many hosts")
    expect(submit().disabled).toBe(true)
    await click(box(2))
    expect(dialog()?.querySelector("[data-share-reason]")).toBeNull()
    expect(submit().disabled).toBe(false)
  })

  it("keeps, and shows, a held host the gateway does not offer now", async () => {
    const sharing = inMemorySharing()
    const store = await render(sharing)
    await sharing.share(
      "a",
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "all", commandHosts: ["offline-peer"] },
      },
      "seed",
    )
    await open(store)
    await click(
      within(find('[data-share-grant="sample-phone"]'), '[data-share-action="change"]'),
    )
    const away = hostBoxes().find((each) =>
      each
        .closest("[data-slot=checkbox-list-row]")
        ?.textContent?.includes("offline-peer"),
    )
    expect(away?.checked).toBe(true)
    expect(away?.closest("[data-slot=checkbox-list-row]")?.textContent).toContain(
      "Not taking your commands now",
    )
    await choose("[data-share-presets]", "No shell")
    const sent: unknown[] = []
    sharing.share = async (_sessionId, request) => {
      sent.push(request)
    }
    await click(submit())
    expect(sent).toEqual([
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "no_shell", commandHosts: ["offline-peer"] },
      },
    ])
  })

  it("starts from nothing for a device without a grant, never another's", async () => {
    const sharing = inMemorySharing()
    const store = await render(sharing)
    await sharing.share(
      "a",
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "all", commandHosts: ["devbox"] },
      },
      "seed",
    )
    await open(store)
    await click(
      within(find('[data-share-grant="sample-phone"]'), '[data-share-action="change"]'),
    )
    expect(dialog()?.querySelector("[data-share-drive]")).not.toBeNull()
    await choose("[data-share-targets]", "tablet")
    expect(dialog()?.querySelector("[data-share-drive]")).toBeNull()
    expect(
      find("[data-share-role]").querySelector('[aria-pressed="true"]')?.textContent,
    ).toBe("Read")
  })

  it("starts a new draft for a new opening", async () => {
    const store = await render(inMemorySharing())
    await open(store)
    await choose("[data-share-targets]", "-phone")
    expect(submit().disabled).toBe(false)
    await open(store, "b")
    expect(submit().disabled).toBe(true)
  })

  it("holds the form while a change is on its way, and starts again after a change made elsewhere", async () => {
    const sharing = inMemorySharing()
    const share = sharing.share
    let release: () => void = () => {}
    sharing.share = (sessionId, request, requestId) =>
      new Promise<void>((resolve) => {
        release = () => resolve(share(sessionId, request, requestId))
      })
    const store = await render(sharing)
    await open(store)
    await choose("[data-share-targets]", "-phone")
    await click(submit())
    expect(dialog()?.getAttribute("data-share-state")).toBe("held")
    expect(find<HTMLFieldSetElement>("[data-share-form]").disabled).toBe(true)
    expect(submit().disabled).toBe(true)
    await act(async () => {
      release()
      await settle()
    })
    expect(find<HTMLFieldSetElement>("[data-share-form]").disabled).toBe(false)
    // Another window changes what the device holds; the next read shows it.
    sharing.share = share
    await share("a", { credentialId: "sample-phone", role: "comment" }, "elsewhere")
    await choose("[data-share-role]", "Drive")
    await act(async () => {
      await store.dispatch(readShares())
    })
    expect(find("[data-share-reason]").getAttribute("data-share-reason")).toBe("changed")
    expect(submit().disabled).toBe(true)
    await click(find('[data-share-action="restart"]'))
    expect(dialog()?.querySelector("[data-share-reason]")).toBeNull()
    expect(
      find("[data-share-role]").querySelector('[aria-pressed="true"]')?.textContent,
    ).toBe("Comment")
  })

  it("says why it waits while another conversation's change is on its way", async () => {
    const sharing = inMemorySharing()
    const share = sharing.share
    let release: () => void = () => {}
    sharing.share = (sessionId, request, requestId) =>
      new Promise<void>((resolve) => {
        release = () => resolve(share(sessionId, request, requestId))
      })
    const store = await render(sharing)
    await open(store)
    await choose("[data-share-targets]", "-phone")
    await click(submit())
    await act(async () => {
      store.dispatch(closeShare())
    })
    await open(store, "b")
    expect(find("[data-share-reason]").getAttribute("data-share-reason")).toBe(
      "elsewhere",
    )
    expect(find("[data-share-reason]").textContent).toContain(
      "One change is made at a time",
    )
    expect(find<HTMLFieldSetElement>("[data-share-form]").disabled).toBe(true)
    await act(async () => {
      release()
      await settle()
    })
    expect(dialog()?.querySelector('[data-share-reason="elsewhere"]')).toBeNull()
    expect(find<HTMLFieldSetElement>("[data-share-form]").disabled).toBe(false)
    // Not this conversation's change: nothing to say of it here.
    expect(find("[data-share-notice]").textContent).toBe("")
  })
})
