// @vitest-environment jsdom
/**
 * The share dialog as drawn: the grants and what each holds, a share and a
 * revoke sent once each and the list read again, every refusal in words and
 * never a wire code, the hosts held to their own bound however full the
 * patterns are, and the notes on what a policy does not stop.
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { makeDesktopStore } from "../../../store"
import { SharingError, type SharingSource } from "../../application/ports"
import { createQuestionFlights } from "../../application/question-flights"
import { inMemorySharing } from "../../adapters/in-memory/in-memory-sharing"
import { closeShare, openShare, readShares } from "../../adapters/store/commands"
import { fakeSource, keptFilter, roomyGrid, settle } from "../../testing"
import { driveNote } from "./share-copy"
import { ShareDialogHost } from "./share-dialog"

/** A fixed clock, so a grant's time is the same on every run. */
const testNow = () => 1_700_000_000_000

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
  // The read's loading text sweeps on Web Animations, which jsdom has not.
  Element.prototype.animate ??= function () {
    return { cancel() {}, finished: Promise.resolve() } as unknown as Animation
  }
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
    questionFlights: createQuestionFlights(),
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
  /share_target_not_paired|share_grant_changed|invalid_request|drive_policy_unenforceable|not-paired|unanswered|this-session|another-session/
/** The source's calls, by method, a read being its grants and its choices. */
const methods = (sharing: ReturnType<typeof inMemorySharing>) =>
  sharing.calls.map((call) => call.method)

describe("the share dialog", () => {
  it("draws nothing until opened, and goes when closed", async () => {
    const store = await render(inMemorySharing({ now: testNow }))
    expect(dialog()).toBeNull()
    await open(store)
    expect(dialog()?.getAttribute("data-share-state")).toBe("read")
    // Nothing shared says itself: no group, no line, no Done beside the ×.
    expect(dialog()?.querySelector("[data-share-grants]")).toBeNull()
    expect(dialog()?.querySelector('[data-share-action="close"]')).toBeNull()
    await act(async () => {
      store.dispatch(closeShare())
    })
    expect(dialog()).toBeNull()
  })

  it("with no linked device, says so in one line and offers the way to link one", async () => {
    const opened: unknown[] = []
    const record = (event: Event) =>
      opened.push(event instanceof CustomEvent ? event.detail : undefined)
    window.addEventListener("nessa:open-settings", record)
    try {
      const store = await render(inMemorySharing({ now: testNow, targets: [] }))
      await open(store)
      expect(find("[data-share-targets]").textContent).toBe(
        "No linked devices yet.Link a Device",
      )
      expect(dialog()?.querySelector('[data-share-action="submit"]')).toBeNull()
      await click(find('[data-share-action="link-device"]'))
      expect(dialog()).toBeNull()
      expect(opened).toEqual(["devices"])
    } finally {
      window.removeEventListener("nessa:open-settings", record)
    }
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
    expect(methods(sharing)).toEqual(["grants", "choices", "share", "grants", "choices"])
    await click(within(row, '[data-share-action="confirm-revoke"]'))
    expect(methods(sharing)).toEqual([
      "grants",
      "choices",
      "share",
      "grants",
      "choices",
      "unshare",
      "grants",
      "choices",
    ])
    // The revoke names the revision the share was answered with.
    expect(sharing.calls.find((call) => call.method === "unshare")).toMatchObject({
      credentialId: "sample-phone",
      expectedPolicyRevision: 1,
    })
    expect(find("[data-share-notice]").textContent).toBe("No longer shared.")
    expect(dialog()?.querySelector("[data-share-grant]")).toBeNull()
  })

  it("says a refusal in words, never its code", async () => {
    const store = await render(
      inMemorySharing({
        now: testNow,
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
      inMemorySharing({
        now: testNow,
        refuse: () => (refuse ? "unanswered" : undefined),
      }),
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
    const sharing = inMemorySharing({ now: testNow })
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
    expect(find("[data-share-notes]").textContent).toBe(driveNote)
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
        // A first grant: built on a list where the device held none.
        expectedPolicyRevision: 0,
      },
    ])
    expect(find('[data-share-grant="sample-phone"]').textContent).toContain(
      "Drive · No shell, commands on me@build.example",
    )
  })

  it("keeps every fitting host available however full the patterns are", async () => {
    const limits = { patterns: 16, hosts: 4, hostBytes: 8 }
    const memory = inMemorySharing({
      now: testNow,
      commandHosts: ["a", "b", "c", "much-too-long"],
    })
    const sharing: SharingSource = {
      ...memory,
      choices: async () => ({ ...(await memory.choices()), limits }),
    }
    const store = await render(sharing)
    await open(store)
    await choose("[data-share-targets]", "-phone")
    await choose("[data-share-role]", "Drive")
    const tooLong = box(3)
    expect(tooLong.closest("[data-slot=checkbox-list-row]")?.textContent).toContain(
      "Name too long for a grant",
    )
    expect(tooLong.disabled || tooLong.getAttribute("aria-disabled") === "true").toBe(
      true,
    )
    for (const box of hostBoxes().slice(0, 3)) await click(box)
    expect(hostBoxes().filter((box) => box.checked)).toHaveLength(3)
    await choose("[data-share-presets]", "Custom")
    const many = (count: number, name: string) =>
      Array.from({ length: count }, (_, at) => `${name}${at}`).join(", ")
    await type(find<HTMLInputElement>('[data-share-pattern="allow"]'), many(16, "Allow"))
    await type(find<HTMLInputElement>('[data-share-pattern="deny"]'), many(16, "Deny"))
    expect(find("[data-share-hosts]").textContent).toContain("3 of 4 selected")
    expect(dialog()?.querySelector("[data-share-reason]")).toBeNull()
    expect(submit().disabled).toBe(false)
  })

  it("keeps, and shows, a held host the gateway does not offer now", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = await render(sharing)
    await sharing.share(
      "a",
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "all", commandHosts: ["offline-peer"] },
        expectedPolicyRevision: 0,
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
      return 2
    }
    await click(submit())
    expect(sent).toEqual([
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "no_shell", commandHosts: ["offline-peer"] },
        expectedPolicyRevision: 1,
      },
    ])
  })

  it("starts from nothing for a device without a grant, never another's", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = await render(sharing)
    await sharing.share(
      "a",
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "all", commandHosts: ["devbox"] },
        expectedPolicyRevision: 0,
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
    const store = await render(inMemorySharing({ now: testNow }))
    await open(store)
    await choose("[data-share-targets]", "-phone")
    expect(submit().disabled).toBe(false)
    await open(store, "b")
    expect(submit().disabled).toBe(true)
  })

  it("holds the form while a change is on its way, and starts again after a change made elsewhere", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const share = sharing.share
    let release: () => void = () => {}
    sharing.share = (sessionId, request, requestId) =>
      new Promise<number>((resolve) => {
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
    await share(
      "a",
      {
        credentialId: "sample-phone",
        role: "comment",
        expectedPolicyRevision: 1, // the revision this window listed
      },
      "elsewhere",
    )
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

  it.each([
    ["a change", true, "change"],
    ["a first share", false, "share"],
    ["a revoke", true, "revoke"],
  ] as const)(
    "holds the list and the form during the read after %s, and once that read fails, until Try Again",
    async (_, seeded, act_) => {
      const sharing = inMemorySharing({ now: testNow })
      if (seeded)
        await sharing.share(
          "a",
          { credentialId: "sample-phone", role: "read", expectedPolicyRevision: 0 },
          "seed",
        )
      const store = await render(sharing)
      await open(store)
      const row = () => find('[data-share-grant="sample-phone"]')
      if (seeded) await click(within(row(), '[data-share-action="change"]'))
      else await choose("[data-share-targets]", "-phone")
      // The read after the change waits, and then fails, until let go.
      const grants = sharing.grants
      let fail: () => void = () => {}
      sharing.grants = () =>
        new Promise((_resolve, reject) => {
          fail = () => reject(new SharingError("busy"))
        })
      if (act_ === "revoke") {
        await click(within(row(), '[data-share-action="revoke"]'))
        await click(within(row(), '[data-share-action="confirm-revoke"]'))
      } else {
        if (act_ === "change") await choose("[data-share-role]", "Comment")
        await click(submit())
      }
      expect(find("[data-share-notice]").getAttribute("data-share-notice")).toBe("done")
      expect(dialog()?.querySelector("[data-share-reason]")).toBeNull()
      await act(async () => {
        fail()
        await settle()
      })
      expect(
        find("[data-share-read-failure]").getAttribute("data-share-read-failure"),
      ).toBe("grants")
      expect(dialog()?.querySelector("[data-share-reason]")).toBeNull()
      expect(dialog()?.querySelector('[data-share-action="restart"]')).toBeNull()
      expect(submit().disabled).toBe(true)
      // The list is stale: held as a whole, so the person cannot build on it.
      expect(dialog()?.getAttribute("data-share-state")).toBe("held")
      expect(find<HTMLFieldSetElement>("[data-share-form]").disabled).toBe(true)
      const draft = store.getState().workspace.share?.draft
      await choose("[data-share-targets]", "-phone")
      const listed = dialog()?.querySelector<HTMLElement>(
        '[data-share-grant="sample-phone"]',
      )
      if (listed) {
        const change = listed.querySelector<HTMLButtonElement>(
          '[data-share-action="change"]',
        )
        const revoke = listed.querySelector<HTMLButtonElement>(
          '[data-share-action="revoke"]',
        )
        expect(change?.disabled ?? true).toBe(true)
        expect(revoke?.disabled).toBe(true)
        if (change) await click(change)
      }
      expect(store.getState().workspace.share?.draft).toEqual(draft)
      // Try Again reads the list after the change; the draft is as the
      // change left it, and agrees with the list.
      sharing.grants = grants
      await click(find('[data-share-action="retry"]'))
      expect(dialog()?.querySelector("[data-share-read-failure]")).toBeNull()
      expect(dialog()?.querySelector("[data-share-reason]")).toBeNull()
      expect(dialog()?.getAttribute("data-share-state")).toBe("read")
      expect(store.getState().workspace.share?.draft).toEqual(draft)
    },
  )

  it("says why it waits while another conversation's change is on its way", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const share = sharing.share
    let release: () => void = () => {}
    sharing.share = (sessionId, request, requestId) =>
      new Promise<number>((resolve) => {
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
      "another-session",
    )
    expect(find("[data-share-reason]").textContent).toContain(
      "One change is made at a time",
    )
    expect(find<HTMLFieldSetElement>("[data-share-form]").disabled).toBe(true)
    await act(async () => {
      release()
      await settle()
    })
    expect(dialog()?.querySelector('[data-share-reason="another-session"]')).toBeNull()
    expect(find<HTMLFieldSetElement>("[data-share-form]").disabled).toBe(false)
    // Not this conversation's change: nothing to say of it here.
    expect(find("[data-share-notice]").textContent).toBe("")
  })

  it("says why it waits while its own conversation's earlier change is on its way", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const share = sharing.share
    let release: () => void = () => {}
    sharing.share = (sessionId, request, requestId) =>
      new Promise<number>((resolve) => {
        release = () => resolve(share(sessionId, request, requestId))
      })
    const store = await render(sharing)
    await open(store)
    await choose("[data-share-targets]", "-phone")
    await click(submit())
    expect(find("[data-share-reason]").getAttribute("data-share-reason")).toBe(
      "this-session",
    )
    await act(async () => {
      store.dispatch(closeShare())
    })
    await open(store)
    const reason = find("[data-share-reason]")
    expect(reason.getAttribute("data-share-reason")).toBe("this-session")
    expect(reason.textContent).toContain("your change to this conversation")
    expect(find<HTMLFieldSetElement>("[data-share-form]").disabled).toBe(true)
    await act(async () => {
      release()
      await settle()
    })
    expect(dialog()?.querySelector("[data-share-reason]")).toBeNull()
    expect(find("[data-share-notice]").textContent).toBe("Shared.")
  })

  it("stops asking about a Revoke once a read lists the grant changed, and asks again about what it holds", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = await render(sharing)
    await sharing.share(
      "a",
      { credentialId: "sample-phone", role: "read", expectedPolicyRevision: 0 },
      "seed",
    )
    await open(store)
    const row = () => find('[data-share-grant="sample-phone"]')
    await click(within(row(), '[data-share-action="revoke"]'))
    expect(row().textContent).toContain("Revoke this device")
    // While it asks, another window changes the grant, and a read lists it.
    await sharing.share(
      "a",
      {
        credentialId: "sample-phone",
        role: "comment",
        expectedPolicyRevision: 1,
      },
      "elsewhere",
    )
    await act(async () => {
      await store.dispatch(readShares())
    })
    // The person was asked about a grant the list no longer holds: not asked now.
    expect(row().textContent).not.toContain("Revoke this device")
    expect(row().textContent).toContain("Comment")
    expect(methods(sharing)).not.toContain("unshare")
    // Asked again, it is about what the device holds now.
    await click(within(row(), '[data-share-action="revoke"]'))
    await click(within(row(), '[data-share-action="confirm-revoke"]'))
    expect(sharing.calls.filter((call) => call.method === "unshare")).toMatchObject([
      { credentialId: "sample-phone", expectedPolicyRevision: 2 },
    ])
    expect(find("[data-share-notice]").getAttribute("data-share-notice")).toBe("done")
  })

  it("stops asking about a Revoke when this window's own change to that device begins", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = await render(sharing)
    await sharing.share(
      "a",
      { credentialId: "sample-phone", role: "read", expectedPolicyRevision: 0 },
      "seed",
    )
    await open(store)
    const row = () => find('[data-share-grant="sample-phone"]')
    await click(within(row(), '[data-share-action="change"]'))
    await click(within(row(), '[data-share-action="revoke"]'))
    expect(row().textContent).toContain("Revoke this device")
    // With the Revoke still asked, the person updates the same device.
    await choose("[data-share-role]", "Comment")
    await click(submit())
    expect(find("[data-share-notice]").getAttribute("data-share-notice")).toBe("done")
    // The ask ended with that change: the old revision can no longer be sent.
    expect(row().textContent).not.toContain("Revoke this device")
    expect(store.getState().workspace.share?.confirming).toBeNull()
    await click(within(row(), '[data-share-action="revoke"]'))
    await click(within(row(), '[data-share-action="confirm-revoke"]'))
    expect(sharing.calls.filter((call) => call.method === "unshare")).toMatchObject([
      { credentialId: "sample-phone", expectedPolicyRevision: 2 },
    ])
    expect(find("[data-share-notice]").getAttribute("data-share-notice")).toBe("done")
  })

  it("says a change refused because another window changed the grant, and starts again from what it holds", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = await render(sharing)
    await sharing.share(
      "a",
      { credentialId: "sample-phone", role: "read", expectedPolicyRevision: 0 },
      "seed",
    )
    await open(store)
    await click(
      within(find('[data-share-grant="sample-phone"]'), '[data-share-action="change"]'),
    )
    await choose("[data-share-role]", "Comment")
    // Another window changes the grant after this one read it.
    await sharing.share(
      "a",
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "all", commandHosts: [] },
        expectedPolicyRevision: 1, // the revision this window listed
      },
      "elsewhere",
    )
    await click(submit())
    const shares = sharing.calls.filter((call) => call.method === "share")
    expect(shares.at(-1)).toMatchObject({ expectedPolicyRevision: 1 })
    expect(find("[data-share-notice]").getAttribute("data-share-notice")).toBe("changed")
    expect(find("[data-share-notice]").textContent).toContain(
      "changed since this list was read",
    )
    // It does not claim the list is current: the read after it may fail.
    expect(find("[data-share-notice]").textContent).toContain("Reading the list again")
    expect(dialog()?.textContent).not.toMatch(wireCodes)
    // Nothing was changed: the list shows the other window's grant.
    expect(find('[data-share-grant="sample-phone"]').textContent).toContain("Drive")
    expect(find("[data-share-reason]").getAttribute("data-share-reason")).toBe("changed")
    expect(submit().disabled).toBe(true)
    await click(find('[data-share-action="restart"]'))
    expect(
      find("[data-share-role]").querySelector('[aria-pressed="true"]')?.textContent,
    ).toBe("Drive")
    await choose("[data-share-role]", "Comment")
    await click(submit())
    expect(sharing.calls.filter((call) => call.method === "share").at(-1)).toMatchObject({
      expectedPolicyRevision: 2,
    })
    expect(find("[data-share-notice]").textContent).toBe("Shared.")
    expect(find('[data-share-grant="sample-phone"]').textContent).toContain("Comment")
  })

  it("keeps the grants and Revoke when the linked devices cannot be read", async () => {
    const sharing = inMemorySharing({
      now: testNow,
      refuse: (method) => (method === "choices" ? "busy" : undefined),
    })
    const store = await render(sharing)
    await sharing.share(
      "a",
      { credentialId: "sample-phone", role: "read", expectedPolicyRevision: 0 },
      "seed",
    )
    await open(store)
    const failure = find("[data-share-read-failure]")
    expect(failure.getAttribute("data-share-read-failure")).toBe("choices")
    expect(failure.textContent).toContain("couldn’t list your linked devices")
    expect(dialog()?.querySelector("[data-share-form]")).toBeNull()
    // Nothing to share with is read, so nothing offers to share.
    expect(dialog()?.querySelector('[data-share-action="submit"]')).toBeNull()
    const row = find('[data-share-grant="sample-phone"]')
    expect(row.querySelector('[data-share-action="change"]')).toBeNull()
    await click(within(row, '[data-share-action="revoke"]'))
    await click(within(row, '[data-share-action="confirm-revoke"]'))
    expect(find("[data-share-notice]").textContent).toBe("No longer shared.")
    expect(dialog()?.querySelector("[data-share-grant]")).toBeNull()
  })

  it("keeps Tab inside the dialog and returns focus to the session row after the menu is gone", async () => {
    const row = document.createElement("button")
    row.dataset.sessionRow = "a"
    row.textContent = "Release notes"
    const menuItem = document.createElement("button")
    menuItem.setAttribute("role", "menuitem")
    menuItem.textContent = "Share…"
    document.body.append(row, menuItem)
    try {
      menuItem.focus()
      const store = await render(inMemorySharing({ now: testNow }))
      await open(store)
      menuItem.remove()
      const panel = dialog()
      expect(panel?.contains(document.activeElement)).toBe(true)
      for (let step = 0; step < 8; step++) {
        document.activeElement?.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }),
        )
        expect(panel?.contains(document.activeElement)).toBe(true)
      }
      await act(async () => {
        document.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Escape",
            code: "Escape",
            bubbles: true,
            cancelable: true,
          }),
        )
      })
      await act(async () => {
        await vi.waitFor(() => {
          if (document.activeElement !== row) throw new Error("focus has not returned")
        })
      })
      expect(dialog()).toBeNull()
    } finally {
      row.remove()
      menuItem.remove()
    }
  })

  it("returns focus to the session row from the close button", async () => {
    const row = document.createElement("button")
    row.dataset.sessionRow = "a"
    document.body.append(row)
    try {
      const store = await render(inMemorySharing({ now: testNow }))
      await open(store)
      await click(find("button[aria-label='Close']"))
      await act(async () => {
        await vi.waitFor(() => {
          if (document.activeElement !== row) throw new Error("focus has not returned")
        })
      })
    } finally {
      row.remove()
    }
  })

  it("leaves focus where a later surface put it", async () => {
    const keeper = document.createElement("button")
    keeper.textContent = "Settings"
    document.body.append(keeper)
    try {
      const store = await render(inMemorySharing({ now: testNow }))
      await open(store)
      // Settings takes focus from its own effect, after this dialog is gone
      // and before the dialog's deferred restore.
      await act(async () => {
        store.dispatch(closeShare())
      })
      keeper.focus()
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0))
      })
      expect(document.activeElement).toBe(keeper)
      expect(dialog()).toBeNull()
    } finally {
      keeper.remove()
    }
  })
})
