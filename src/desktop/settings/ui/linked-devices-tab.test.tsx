// @vitest-environment jsdom
/**
 * Linked devices as drawn (L1, and the requests the reducer names, once,
 * under StrictMode).
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { ClockProvider } from "../../../clock"
import type {
  LinkedDevicesConnection,
  LinkedDevicesGateway,
} from "../adapters/linked-devices-gateway"
import type { Outcome, ReadValue } from "../model/linked-devices"
import type { TunnelChange, TunnelRead } from "../model/tunnel"
import { LinkedDevicesProvider, LinkedDevicesTab } from "./linked-devices-tab"

let root: Root
let host: HTMLDivElement
// The window's injected clock, as composition gives it; a test moves it.
let clockOffset = 0
const now = () => Date.now() + clockOffset

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  if (typeof window.matchMedia !== "function")
    window.matchMedia = ((query: string) => ({
      matches: true,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as typeof window.matchMedia
  clockOffset = 0
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const tunnelOff: TunnelRead = {
  enabled: false,
  mode: "quick",
  hasToken: false,
  ingress: "127.0.0.1:47651",
  status: "off",
}

function gateway(
  read: () => Promise<Outcome<ReadValue>>,
  tunnel: () => Promise<Outcome<TunnelRead>> = async () => ({
    ok: true,
    value: tunnelOff,
  }),
): LinkedDevicesGateway & {
  switchWaitMs: number
  creates: number
  linkings: boolean[]
  tunnels: TunnelChange[]
  tell: (connection: LinkedDevicesConnection) => void
} {
  // Every part of the tab that follows the connection hears each change.
  const followers = new Set<(connection: LinkedDevicesConnection) => void>()
  const gate = {
    pollMs: 0,
    switchWaitMs: 60_000,
    creates: 0,
    linkings: [] as boolean[],
    tunnels: [] as TunnelChange[],
    tell: (connection: LinkedDevicesConnection) => {
      for (const follower of [...followers]) follower(connection)
    },
    follow(handler: (connection: LinkedDevicesConnection) => void) {
      followers.add(handler)
      handler({ type: "connected" })
      return () => {
        followers.delete(handler)
      }
    },
    read,
    tunnel,
    setTunnel: async (change: TunnelChange) => {
      gate.tunnels.push(change)
      return {
        ok: true as const,
        value: {
          enabled: change.enabled,
          mode: change.mode ?? ("quick" as const),
          applies: "restarting" as const,
        },
      }
    },
    create: async () => {
      gate.creates += 1
      return {
        ok: true as const,
        value: {
          code: "ABCD-2345",
          enrollment: {
            invitationId: "00112233445566778899aabbccddeeff",
            phase: "available" as const,
            expiresAtMs: Date.now() + 60_000,
          },
        },
      }
    },
    cancel: async () => ({
      ok: true as const,
      value: {
        invitationId: "x",
        phase: "terminal" as const,
        expiresAtMs: 0,
        terminal: "cancelled" as const,
      },
    }),
    refresh: async () => ({ ok: false as const, failure: { kind: "busy" as const } }),
    approve: async () => ({
      ok: true as const,
      value: {
        enrollment: {
          invitationId: "00112233445566778899aabbccddeeff",
          phase: "approved" as const,
          expiresAtMs: Date.now() + 60_000,
          deviceKey: "11".repeat(32),
          fingerprint: "182ff9da701fd144e2fd2cd41da8ddba979eb01b2bf7fcc3376f4b1b2ecee4e7",
        },
        activationStopped: "retryable" as const,
      },
    }),
    deny: async () => ({
      ok: true as const,
      value: { invitationId: "x", phase: "terminal" as const, expiresAtMs: 0 },
    }),
    status: async () => ({
      ok: true as const,
      value: { invitationId: "x", phase: "available" as const, expiresAtMs: 0 },
    }),
    revoke: async () => ({ ok: true as const, value: { credentialId: "device-1" } }),
    setLinking: async (enabled: boolean) => {
      gate.linkings.push(enabled)
      return { ok: true as const, value: { enabled, applies: "restarting" as const } }
    },
  }
  return gate
}

async function mount(node: React.ReactNode) {
  await act(async () => {
    root.render(
      <StrictMode>
        <ClockProvider now={now}>{node}</ClockProvider>
      </StrictMode>,
    )
  })
}

/** Run both deferred close-focus callbacks without advancing the product clock. */
async function closePairingDialog(control: string) {
  // Radix closes its focus scope on one timer; the kit's body fallback runs
  // on a second. One virtual millisecond admits that nested zero-delay timer,
  // but does not admit a genuinely delayed fallback or release a held read.
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] })
  try {
    const button = document.querySelector<HTMLButtonElement>(
      `[data-slot='dialog'] ${control}`,
    )
    expect(button).not.toBeNull()
    expect(button?.disabled).toBe(false)
    await act(async () => button?.click())
    await act(async () => vi.advanceTimersByTimeAsync(1))
  } finally {
    vi.useRealTimers()
  }
}

describe("Linked devices", () => {
  it("L1: with no gateway the page is pending and offers no pairing", async () => {
    await mount(<LinkedDevicesTab />)
    const page = host.querySelector("[data-linked]")
    expect(page?.getAttribute("data-linked")).toBe("pending")
    expect(page?.hasAttribute("data-pending")).toBe(true)
    expect(page?.textContent).toContain("Not available yet")
    expect(host.querySelector("[data-linked-action='pair']")).toBeNull()
  })

  it("a failed first read is not still checking", async () => {
    const gate = gateway(async () => ({ ok: false, failure: { kind: "unanswered" } }))
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    expect(host.textContent).toContain("No answer. The list is read again")
    expect(host.textContent).not.toContain("Refresh")
    expect(host.textContent).not.toContain("Checking whether linking is on")
    expect(host.querySelector("[data-linked-action='pair']")).toBeNull()
  })

  it("L2: linking off is a switch that can be turned, with no config hint, no group title and no pair button", async () => {
    const gate = gateway(async () => ({ ok: true, value: { kind: "off" } }))
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    expect(host.querySelector("[data-linked]")?.getAttribute("data-linked")).toBe("off")
    const toggle = host.querySelector<HTMLButtonElement>("[role='switch']")
    expect(toggle?.getAttribute("aria-checked")).toBe("false")
    expect(toggle?.disabled).toBe(false)
    expect(host.textContent).not.toContain("config.json")
    expect(host.textContent).toContain("Changing this restarts the gateway")
    expect(host.querySelector('[data-slot="settings-group-title"]')).toBeNull()
    expect(host.textContent).not.toContain("pairing_not_configured")
    expect(host.querySelector("[data-linked-action='pair']")).toBeNull()
  })

  it("L23: flipping the switch sends one change and holds it through the restart", async () => {
    let on = false
    const gate = gateway(async () =>
      on
        ? { ok: true, value: { kind: "on", enrollments: [], devices: [] } }
        : { ok: true, value: { kind: "off" } },
    )
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    const toggle = () => host.querySelector<HTMLButtonElement>("[role='switch']")
    await act(async () => toggle()?.click())
    expect(gate.linkings).toEqual([true])
    expect(toggle()?.getAttribute("aria-checked")).toBe("true")
    expect(toggle()?.disabled).toBe(true)
    expect(host.textContent).toContain("Restarting the gateway")
    on = true
    await act(async () => gate.tell({ type: "unreachable" }))
    expect(host.textContent).not.toContain("Cannot reach the gateway")
    await act(async () => gate.tell({ type: "connected" }))
    expect(host.querySelector("[data-linked]")?.getAttribute("data-linked")).toBe("on")
    expect(toggle()?.disabled).toBe(false)
    expect(host.textContent).not.toContain("Restarting the gateway")
    expect(host.querySelector("[data-linked-action='pair']")).not.toBeNull()
  })

  it("L23: turning linking off does not say the gateway cannot be reached while it restarts", async () => {
    let on = true
    const gate = gateway(async () =>
      on
        ? { ok: true, value: { kind: "on", enrollments: [], devices: [] } }
        : { ok: true, value: { kind: "off" } },
    )
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    const toggle = () => host.querySelector<HTMLButtonElement>("[role='switch']")
    await act(async () => toggle()?.click())
    expect(gate.linkings).toEqual([false])
    on = false
    await act(async () => gate.tell({ type: "unreachable" }))
    expect(toggle()?.getAttribute("aria-checked")).toBe("false")
    expect(host.textContent).not.toContain("Cannot reach the gateway")
    await act(async () => gate.tell({ type: "connected" }))
    expect(host.querySelector("[data-linked]")?.getAttribute("data-linked")).toBe("off")
  })

  it("L24: a gateway not back within the wait lets the switch go and says the change is saved", async () => {
    let on = false
    const gate = gateway(async () =>
      on
        ? { ok: true, value: { kind: "on", enrollments: [], devices: [] } }
        : { ok: true, value: { kind: "off" } },
    )
    gate.switchWaitMs = 20
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    const toggle = () => host.querySelector<HTMLButtonElement>("[role='switch']")
    await act(async () => toggle()?.click())
    await act(async () => gate.tell({ type: "unreachable" }))
    await act(async () => new Promise((done) => setTimeout(done, 60)))
    expect(toggle()?.getAttribute("aria-checked")).toBe("false")
    expect(host.textContent).toContain("has not come back yet")
    expect(host.textContent).not.toContain("Restarting the gateway")
    on = true
    await act(async () => gate.tell({ type: "connected" }))
    expect(host.querySelector("[data-linked]")?.getAttribute("data-linked")).toBe("on")
    expect(host.textContent).not.toContain("has not come back yet")
  })

  it("L5: Pair sends one create under StrictMode and shows the code and one orb in a modal", async () => {
    let listed = false
    const gate = gateway(async () => {
      if (!listed) {
        listed = true
        return { ok: true, value: { kind: "on", enrollments: [], devices: [] } }
      }
      return {
        ok: true,
        value: {
          kind: "on",
          enrollments: [
            {
              invitationId: "00112233445566778899aabbccddeeff",
              phase: "available",
              expiresAtMs: Date.now() + 60_000,
            },
          ],
          devices: [],
        },
      }
    })
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    const pair = host.querySelector<HTMLButtonElement>("[data-linked-action='pair']")
    expect(pair?.disabled).toBe(false)
    // WebKit pointer clicks leave focus on the previous control. Closing the
    // pairing dialog must still return to its row, not that unrelated control.
    host.tabIndex = -1
    host.focus()
    await act(async () => pair?.click())
    expect(gate.creates).toBe(1)
    const dialog = document.querySelector("[data-slot='dialog']")
    expect(dialog?.textContent).toContain("Pair a device")
    expect(dialog?.querySelector("[data-slot='pairing-code-value']")?.textContent).toBe(
      "ABCD2345",
    )
    expect(dialog?.querySelectorAll("[data-slot='signal-orb']")).toHaveLength(1)
    expect(document.querySelector("[data-slot='qr-orb']")).toBeNull()
    // The page keeps one row, which no longer offers Pair a device.
    expect(host.querySelector("[data-slot='pairing-code']")).toBeNull()
    expect(host.querySelector("[data-linked-action='pair']")).toBeNull()

    // Closing the modal leaves the code open; the row shows it again.
    await closePairingDialog("[data-slot='dialog-close-button']")
    expect(document.querySelector("[data-slot='dialog']")).toBeNull()
    expect(gate.creates).toBe(1)
    const show = host.querySelector<HTMLButtonElement>("[data-linked-action='show']")
    expect(show).not.toBeNull()
    // The Pair button that opened it is gone: focus goes to the row's Show code,
    // once Radix's own restore has left it resting.
    expect(document.activeElement).toBe(show)
    await act(async () => show?.click())
    expect(
      document.querySelector("[data-slot='dialog'] [data-slot='pairing-code-value']")
        ?.textContent,
    ).toBe("ABCD2345")
  })

  it("cancelling the code in the modal closes it and offers Pair a device again", async () => {
    let reads = 0
    const available = {
      invitationId: "00112233445566778899aabbccddeeff",
      phase: "available" as const,
      expiresAtMs: Date.now() + 60_000,
    }
    const gate = gateway(async () => {
      reads += 1
      return reads === 2
        ? { ok: true, value: { kind: "on", enrollments: [available], devices: [] } }
        : { ok: true, value: { kind: "on", enrollments: [], devices: [] } }
    })
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    await act(async () =>
      host.querySelector<HTMLButtonElement>("[data-linked-action='pair']")?.click(),
    )
    await closePairingDialog("[data-linked-action='cancel']")
    expect(document.querySelector("[data-slot='dialog']")).toBeNull()
    expect(host.querySelector("[data-linked-action='pair']")).not.toBeNull()
    expect(document.activeElement?.getAttribute("data-linked-action")).toBe("pair")
  })

  it("focus waits on the row while the read after a cancel is out, then takes Pair", async () => {
    let reads = 0
    let answerRead: (() => void) | undefined
    const available = {
      invitationId: "00112233445566778899aabbccddeeff",
      phase: "available" as const,
      expiresAtMs: Date.now() + 60_000,
    }
    const gate = gateway(async () => {
      reads += 1
      if (reads === 3) await new Promise<void>((done) => (answerRead = done))
      return reads === 2
        ? { ok: true, value: { kind: "on", enrollments: [available], devices: [] } }
        : { ok: true, value: { kind: "on", enrollments: [], devices: [] } }
    })
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    await act(async () =>
      host.querySelector<HTMLButtonElement>("[data-linked-action='pair']")?.click(),
    )
    await closePairingDialog("[data-linked-action='cancel']")
    // The dialog's return-focus fallback can follow its close timer. Keep the
    // read held until the actual parked focus is observed, not one timer turn.
    await act(async () =>
      vi.waitFor(() => {
        expect(document.querySelector("[data-slot='dialog']")).toBeNull()
        expect(document.activeElement?.hasAttribute("data-pairing-row")).toBe(true)
      }),
    )
    await act(async () => answerRead?.())
    expect(document.activeElement?.getAttribute("data-linked-action")).toBe("pair")
  })

  it("a live code whose deadline the injected clock passes drops its orb", async () => {
    let reads = 0
    const gate = gateway(async () => {
      reads += 1
      return {
        ok: true,
        value: {
          kind: "on",
          enrollments:
            reads === 1
              ? []
              : [
                  {
                    invitationId: "00112233445566778899aabbccddeeff",
                    phase: "available",
                    expiresAtMs: Date.now() + 60_000,
                  },
                ],
          devices: [],
        },
      }
    })
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    await act(async () =>
      host.querySelector<HTMLButtonElement>("[data-linked-action='pair']")?.click(),
    )
    const dialog = () => document.querySelector("[data-slot='dialog']")
    expect(dialog()?.querySelectorAll("[data-slot='signal-orb']")).toHaveLength(1)
    // Only the window's clock moves past the 60 s deadline; the next second's
    // tick reads it and the orb goes, with the code still drawn.
    clockOffset = 61_000
    await act(async () => new Promise((done) => setTimeout(done, 1_100)))
    expect(dialog()?.querySelector("[data-slot='signal-orb']")).toBeNull()
    expect(dialog()?.querySelector("[data-slot='pairing-code-value']")?.textContent).toBe(
      "ABCD2345",
    )
  })

  it("a failed New code is said in the modal, and an expired code draws no orb", async () => {
    let reads = 0
    const gate = gateway(async () => {
      reads += 1
      return {
        ok: true,
        value: {
          kind: "on",
          enrollments:
            reads === 1
              ? []
              : [
                  {
                    invitationId: "00112233445566778899aabbccddeeff",
                    phase: "available",
                    expiresAtMs: Date.now() - 1,
                  },
                ],
          devices: [],
        },
      }
    })
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    await act(async () =>
      host.querySelector<HTMLButtonElement>("[data-linked-action='pair']")?.click(),
    )
    const dialog = () => document.querySelector("[data-slot='dialog']")
    expect(dialog()?.querySelector("[data-slot='signal-orb']")).toBeNull()
    await act(async () =>
      dialog()
        ?.querySelector<HTMLButtonElement>("[data-linked-action='refresh']")
        ?.click(),
    )
    expect(dialog()?.querySelector("[data-linked-notice]")?.textContent).toContain(
      "Another pairing is in progress",
    )
    expect(host.querySelector("[data-linked-notice]")).toBeNull()
  })

  it("a device that used the code is approved from the modal, not twice on the page", async () => {
    let reads = 0
    const claimed = {
      invitationId: "00112233445566778899aabbccddeeff",
      phase: "claimed" as const,
      expiresAtMs: Date.now() + 60_000,
      deviceKey: "11".repeat(32),
      fingerprint: "182ff9da701fd144e2fd2cd41da8ddba979eb01b2bf7fcc3376f4b1b2ecee4e7",
    }
    const gate = gateway(async () => {
      reads += 1
      return reads === 1
        ? { ok: true, value: { kind: "on", enrollments: [], devices: [] } }
        : { ok: true, value: { kind: "on", enrollments: [claimed], devices: [] } }
    })
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    await act(async () =>
      host.querySelector<HTMLButtonElement>("[data-linked-action='pair']")?.click(),
    )
    const dialog = document.querySelector("[data-slot='dialog']")
    expect(
      dialog?.querySelector("[data-slot='pairing-code']")?.getAttribute("data-state"),
    ).toBe("used")
    expect(dialog?.querySelector("[data-slot='signal-orb']")).toBeNull()
    expect(dialog?.querySelector("[data-linked-action='approve']")).not.toBeNull()
    expect(host.querySelector("[data-linked-action='approve']")).toBeNull()
  })

  it("L21: approve, deny and revoke rest while the gateway cannot be reached", async () => {
    const gate = gateway(async () => ({
      ok: true,
      value: {
        kind: "on",
        enrollments: [
          {
            invitationId: "00112233445566778899aabbccddeeff",
            phase: "claimed",
            expiresAtMs: Date.now() + 60_000,
            deviceKey: "11".repeat(32),
            fingerprint:
              "182ff9da701fd144e2fd2cd41da8ddba979eb01b2bf7fcc3376f4b1b2ecee4e7",
          },
        ],
        devices: [
          { credentialId: "device-1", issuedAt: 1_700_000_000, fullAccess: false },
        ],
      },
    }))
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    const enabled = (action: string) =>
      host.querySelector<HTMLButtonElement>(`[data-linked-action='${action}']`)?.disabled
    expect(enabled("approve")).toBe(false)
    expect(enabled("revoke")).toBe(false)
    await act(async () => gate.tell({ type: "unreachable" }))
    expect(enabled("approve")).toBe(true)
    expect(enabled("deny")).toBe(true)
    expect(enabled("revoke")).toBe(true)
    expect(host.textContent).toContain("Cannot reach the gateway")
  })

  it("L11: Approve stays after a retryable stop, and the sentence is not a wire code", async () => {
    const gate = gateway(async () => ({
      ok: true,
      value: {
        kind: "on",
        enrollments: [
          {
            invitationId: "00112233445566778899aabbccddeeff",
            phase: "claimed",
            expiresAtMs: Date.now() + 60_000,
            deviceKey: "11".repeat(32),
            fingerprint:
              "182ff9da701fd144e2fd2cd41da8ddba979eb01b2bf7fcc3376f4b1b2ecee4e7",
          },
        ],
        devices: [],
      },
    }))
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    const approve = host.querySelector<HTMLButtonElement>(
      "[data-linked-action='approve']",
    )
    await act(async () => approve?.click())
    expect(host.textContent).toContain("Try again")
    expect(host.textContent).not.toContain("retryable")
    expect(host.querySelector("[data-linked-action='approve']")).not.toBeNull()
    expect(host.querySelector("[data-slot='key-fingerprint']")).not.toBeNull()
  })

  it("L15: revoke asks first and sends nothing until it is confirmed", async () => {
    let revokes = 0
    const gate = gateway(async () => ({
      ok: true,
      value: {
        kind: "on",
        enrollments: [],
        devices: [
          { credentialId: "device-1", issuedAt: 1_700_000_000, fullAccess: false },
        ],
      },
    }))
    gate.revoke = async () => {
      revokes += 1
      return { ok: true, value: { credentialId: "device-1" } }
    }
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    await act(async () =>
      host.querySelector<HTMLButtonElement>("[data-linked-action='revoke']")?.click(),
    )
    expect(revokes).toBe(0)
    expect(host.textContent).toContain("Revoke this device?")
    await act(async () =>
      host
        .querySelector<HTMLButtonElement>("[data-linked-action='confirm-revoke']")
        ?.click(),
    )
    expect(revokes).toBe(1)
    expect(host.textContent).toContain("next time it reads")
  })
})

describe("Web tunnel", () => {
  const linkedOn = async () => ({
    ok: true as const,
    value: { kind: "on" as const, enrollments: [], devices: [] },
  })
  const tunnelSwitch = () =>
    host.querySelector<HTMLButtonElement>("[data-tunnel] [role='switch']")

  it("is drawn only while linking is on", async () => {
    const gate = gateway(async () => ({ ok: true, value: { kind: "off" } }))
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    expect(host.querySelector("[data-tunnel]")).toBeNull()
  })

  it("turns a quick tunnel on with one change and shows its address once ready", async () => {
    let read: TunnelRead = tunnelOff
    const gate = gateway(linkedOn, async () => ({ ok: true, value: read }))
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    expect(host.querySelector("[data-tunnel]")?.getAttribute("data-tunnel-status")).toBe(
      "off",
    )
    expect(host.textContent).toContain("only on the same network")
    expect(host.textContent).toContain("changes whenever cloudflared has to start again")
    expect(host.querySelector("[data-tunnel-field]")).toBeNull()
    await act(async () => tunnelSwitch()?.click())
    expect(gate.tunnels).toEqual([{ enabled: true, mode: "quick" }])
    expect(tunnelSwitch()?.getAttribute("aria-checked")).toBe("true")
    expect(tunnelSwitch()?.disabled).toBe(true)
    read = {
      ...tunnelOff,
      enabled: true,
      status: "ready",
      address: "https://a-b.trycloudflare.com",
    }
    await act(async () => gate.tell({ type: "unreachable" }))
    await act(async () => gate.tell({ type: "connected" }))
    expect(host.querySelector("[data-tunnel-address]")?.textContent).toBe(
      "https://a-b.trycloudflare.com",
    )
    expect(tunnelSwitch()?.disabled).toBe(false)
    expect(host.textContent).not.toContain("Restarting the gateway")
  })

  it("a named tunnel takes a hostname and a token, shows where to route it, and never shows the token back", async () => {
    const gate = gateway(linkedOn, async () => ({
      ok: true,
      value: {
        ...tunnelOff,
        mode: "named",
        hostname: "https://n.example",
        hasToken: true,
      },
    }))
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    const field = (name: string) =>
      host.querySelector<HTMLInputElement>(`[data-tunnel-field='${name}']`)
    expect(field("hostname")?.value).toBe("https://n.example")
    expect(field("token")?.value).toBe("")
    expect(field("token")?.type).toBe("password")
    expect(field("token")?.placeholder).toContain("Saved")
    expect(host.textContent).toContain("route this hostname to http://127.0.0.1:47651")
    expect(host.textContent).toContain("Its address never changes")
    await act(async () => tunnelSwitch()?.click())
    expect(gate.tunnels).toEqual([
      { enabled: true, mode: "named", hostname: "https://n.example" },
    ])
  })
})
