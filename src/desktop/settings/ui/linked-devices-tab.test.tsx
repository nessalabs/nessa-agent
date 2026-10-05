// @vitest-environment jsdom
/**
 * Linked devices as drawn (L1, and the requests the reducer names, once,
 * under StrictMode).
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import type {
  LinkedDevicesConnection,
  LinkedDevicesGateway,
} from "../adapters/linked-devices-gateway"
import type { Outcome, ReadValue } from "../model/linked-devices"
import { LinkedDevicesProvider, LinkedDevicesTab } from "./linked-devices-tab"

let root: Root
let host: HTMLDivElement

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
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

function gateway(read: () => Promise<Outcome<ReadValue>>): LinkedDevicesGateway & {
  creates: number
} {
  const gate = {
    pollMs: 0,
    creates: 0,
    follow(handler: (connection: { type: "connected" }) => void) {
      handler({ type: "connected" })
      return () => {}
    },
    read,
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
  }
  return gate
}

async function mount(node: React.ReactNode) {
  await act(async () => {
    root.render(<StrictMode>{node}</StrictMode>)
  })
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
    expect(host.textContent).toContain("No answer")
    expect(host.textContent).not.toContain("Checking whether linking is on")
    expect(host.querySelector("[data-linked-action='pair']")).toBeNull()
  })

  it("L2: linking off is a disabled switch and the config hint, with no pair button", async () => {
    const gate = gateway(async () => ({ ok: true, value: { kind: "off" } }))
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    expect(host.querySelector("[data-linked]")?.getAttribute("data-linked")).toBe("off")
    const toggle = host.querySelector<HTMLButtonElement>("[role='switch']")
    expect(toggle?.getAttribute("aria-checked")).toBe("false")
    expect(toggle?.disabled).toBe(true)
    expect(host.textContent).toContain("127.0.0.1:47650")
    expect(host.textContent).not.toContain("pairing_not_configured")
    expect(host.querySelector("[data-linked-action='pair']")).toBeNull()
  })

  it("L5: Pair sends one create under StrictMode and shows the code and both orbs", async () => {
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
    await act(async () => pair?.click())
    expect(gate.creates).toBe(1)
    expect(host.querySelector("[data-slot='pairing-code-value']")?.textContent).toBe(
      "ABCD2345",
    )
    expect(host.querySelector("[data-slot='signal-orb']")).not.toBeNull()
    expect(host.querySelector("[data-slot='qr-orb']")).not.toBeNull()
    expect(document.activeElement?.getAttribute("data-linked-action")).toBe("cancel")
  })

  it("L21: approve, deny and revoke rest while the gateway cannot be reached", async () => {
    let tell: ((connection: LinkedDevicesConnection) => void) | undefined
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
        devices: [{ credentialId: "device-1", issuedAt: 1_700_000_000 }],
      },
    }))
    gate.follow = (handler) => {
      tell = handler
      handler({ type: "connected" })
      return () => {}
    }
    await mount(
      <LinkedDevicesProvider gateway={gate}>
        <LinkedDevicesTab />
      </LinkedDevicesProvider>,
    )
    const enabled = (action: string) =>
      host.querySelector<HTMLButtonElement>(`[data-linked-action='${action}']`)?.disabled
    expect(enabled("approve")).toBe(false)
    expect(enabled("revoke")).toBe(false)
    await act(async () => tell?.({ type: "unreachable" }))
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
        devices: [{ credentialId: "device-1", issuedAt: 1_700_000_000 }],
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
