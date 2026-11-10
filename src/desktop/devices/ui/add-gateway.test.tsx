// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { PairingApi } from "@nessa/client"
import type { GatewayPairing, PairingScope } from "../adapters/gateway-pairing"
import * as deviceKey from "../../settings/model/device-key"
import { AddGateway } from "./add-gateway"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
function present<T>(value: T | undefined | null): T {
  if (value == null) throw new Error("Expected test element")
  return value
}

let root: Root
let container: HTMLDivElement
let controller: AbortSignal
const saved = (samePerson: boolean | undefined = true) => ({
  invitationId: Array(16).fill(1),
  consentId: Array(16).fill(2),
  generation: 1,
  class: "peer-gateway-conversation-read",
  grant: {
    action: "conversation.read",
    resource: { organizationId: "org", id: "gateway" },
  },
  createdAtMs: 1,
  expiresAtMs: 600_001,
  phase: "available" as const,
  cleanupPending: false,
  ...(samePerson === undefined ? {} : { samePerson }),
})
let scope: PairingScope
let gateway: GatewayPairing
beforeEach(() => {
  vi.useFakeTimers()
  window.matchMedia ??= (() => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
  })) as never
  scope = {
    close: vi.fn(),
    pairing: {
      pending: vi.fn(async () => ({ items: [] })),
      create: vi.fn(async () => ({ code: "ABCD-EFGH", status: saved() })),
      status: vi.fn(async () => saved()),
      approve: vi.fn(),
      cancel: vi.fn(async () => ({ ...saved(), phase: "terminal" as const })),
    } as unknown as PairingApi,
  }
  gateway = {
    after: (ms, run) => {
      const timer = setTimeout(run, ms)
      return () => clearTimeout(timer)
    },
    open: vi.fn(async (_token, signal) => {
      controller = signal
      return scope
    }),
  }
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})
afterEach(async () => {
  await act(async () => root.unmount())
  vi.restoreAllMocks()
  vi.useRealTimers()
  document.body.replaceChildren()
})
async function mount() {
  await act(async () =>
    root.render(<AddGateway gateway={gateway} onBack={() => {}} port={47651} />),
  )
}
async function click(text: string) {
  const button = [...container.querySelectorAll("button")].find(
    (each) => each.textContent === text,
  )
  expect(button, text).toBeDefined()
  await act(async () => present(button).click())
}
async function chooseOwner() {
  await act(async () =>
    present(container.querySelector<HTMLInputElement>('[type="checkbox"]')).click(),
  )
  await act(async () => {
    const input = present(container.querySelector<HTMLInputElement>('[type="password"]'))
    present(
      present(Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")).set,
    ).call(input, "owner-secret")
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}

it("does not create before the choice, authenticates explicitly, and renders the canonical saved choice", async () => {
  await mount()
  expect(scope.pairing.create).not.toHaveBeenCalled()
  await chooseOwner()
  await click("Continue")
  expect(gateway.open).toHaveBeenCalledWith("owner-secret", expect.any(AbortSignal))
  expect(container.querySelector('[type="password"]')).toBeNull()
  expect(scope.pairing.create).not.toHaveBeenCalled()
  // A returned false is rendered as false, not overwritten by the earlier toggle.
  vi.mocked(scope.pairing.create).mockResolvedValue({
    code: "ABCD-EFGH",
    status: saved(false),
  })
  await click("Create invitation")
  expect(scope.pairing.create).toHaveBeenCalledExactlyOnceWith({
    enrollee: "gateway",
    samePerson: true,
  })
  expect(container.querySelector("[data-gateway-choice]")?.textContent).toContain(
    "no person link",
  )
  expect(container.querySelector('[type="checkbox"]')).toBeNull()
  expect(container.textContent).toContain("ABCD-EFGH")
  expect(container.querySelector("pre")?.textContent).toContain("GATEWAY_IP:47651")
  await click("End pairing sign-in")
  expect(controller.aborted).toBe(true)
  expect(scope.close).toHaveBeenCalled()
  expect(container.textContent).not.toContain("ABCD-EFGH")
  expect(container.textContent).not.toContain("owner-secret")
})

it("creates false on the panel and requires a successful pending read after a lost reply", async () => {
  await mount()
  await click("Continue")
  expect(gateway.open).toHaveBeenCalledWith(undefined, expect.any(AbortSignal))
  vi.mocked(scope.pairing.create).mockRejectedValueOnce(new Error("lost"))
  await click("Create invitation")
  const create = present(
    [...container.querySelectorAll("button")].find(
      (each) => each.textContent === "Create invitation",
    ),
  )
  expect(create.disabled).toBe(true)
  expect(scope.pairing.create).toHaveBeenCalledExactlyOnceWith({
    enrollee: "gateway",
    samePerson: false,
  })
  await click("Read pending invitations")
  expect(create.disabled).toBe(false)
  expect(scope.pairing.create).toHaveBeenCalledTimes(1)
})

it("recovers an immutable saved choice without regenerating a code or inferring a legacy value", async () => {
  vi.mocked(scope.pairing.pending).mockResolvedValue({
    items: [
      saved(false),
      { ...saved(), invitationId: Array(16).fill(3), samePerson: undefined },
    ],
  })
  await mount()
  await click("Continue")
  const resume = present(
    [...container.querySelectorAll("button")].find((each) =>
      each.textContent?.includes("no person link — Waiting for the other gateway"),
    ),
  )
  await act(async () => resume.click())
  expect(container.querySelector("[data-gateway-choice]")?.textContent).toContain(
    "no person link",
  )
  expect(container.querySelector("[data-gateway-code]")).toBeNull()
  expect(scope.pairing.create).not.toHaveBeenCalled()
})

it("closes a late sign-in after unmount and never starts a pending read", async () => {
  let finish!: (value: PairingScope) => void
  gateway.open = vi.fn<GatewayPairing["open"]>((_token, signal) => {
    controller = signal
    return new Promise((resolve) => {
      finish = resolve
    })
  })
  await mount()
  await click("Continue")
  await act(async () => root.unmount())
  expect(controller.aborted).toBe(true)
  await act(async () => finish(scope))
  expect(scope.close).toHaveBeenCalledOnce()
  expect(scope.pairing.pending).not.toHaveBeenCalled()
})

it("retires sign-in at its deadline while leaving the invitation recoverable", async () => {
  await mount()
  await click("Continue")
  await click("Create invitation")
  await act(async () => vi.advanceTimersByTimeAsync(10 * 60 * 1000))
  expect(controller.aborted).toBe(true)
  expect(scope.close).toHaveBeenCalled()
  expect(scope.pairing.cancel).not.toHaveBeenCalled()
  expect(container.querySelector("[data-gateway-code]")).toBeNull()
  expect(container.textContent).toContain("Pairing sign-in ended")
})

it("retires the privileged scope on pagehide even without a React unmount", async () => {
  await mount()
  await chooseOwner()
  await click("Continue")
  await act(async () => window.dispatchEvent(new Event("pagehide")))
  expect(controller.aborted).toBe(true)
  expect(scope.close).toHaveBeenCalledOnce()
})

it("refuses creation while the initial pending read is unavailable", async () => {
  vi.mocked(scope.pairing.pending).mockRejectedValueOnce(new Error("read unavailable"))
  await mount()
  await click("Continue")
  const create = present(
    [...container.querySelectorAll("button")].find(
      (each) => each.textContent === "Create invitation",
    ),
  )
  expect(create.disabled).toBe(true)
  expect(scope.pairing.create).not.toHaveBeenCalled()
  await click("Read pending invitations")
  expect(create.disabled).toBe(false)
})

it("resumes Human invitations without asking their immutable choice again", async () => {
  vi.mocked(scope.pairing.pending).mockResolvedValue({ items: [saved()] })
  await mount()
  await click("Resume owner invitations")
  expect(container.querySelector('[type="checkbox"]')).toBeNull()
  await act(async () => {
    const input = present(container.querySelector<HTMLInputElement>('[type="password"]'))
    present(
      present(Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")).set,
    ).call(input, "owner-secret")
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
  await click("Continue")
  expect(gateway.open).toHaveBeenCalledWith("owner-secret", expect.any(AbortSignal))
  expect(
    [...container.querySelectorAll("button")].some(
      (each) => each.textContent === "Create invitation",
    ),
  ).toBe(false)
  expect(container.textContent).toContain("same person — Waiting for the other gateway")
  expect(scope.pairing.create).not.toHaveBeenCalled()
})

it("serializes double create and ignores a late answer after sign-out", async () => {
  let finish!: (value: Awaited<ReturnType<PairingApi["create"]>>) => void
  vi.mocked(scope.pairing.create).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  await mount()
  await click("Continue")
  const create = present(
    [...container.querySelectorAll("button")].find(
      (each) => each.textContent === "Create invitation",
    ),
  )
  await act(async () => {
    create.click()
    create.click()
  })
  expect(scope.pairing.create).toHaveBeenCalledOnce()
  await click("End pairing sign-in")
  await act(async () => finish({ code: "ABCD-EFGH", status: saved() }))
  expect(container.querySelector("[data-gateway-code]")).toBeNull()
  expect(container.textContent).not.toContain("ABCD-EFGH")
  expect(scope.pairing.cancel).not.toHaveBeenCalled()
})

it("keeps Active with owed attribution retryable and closes after the exact retry completes", async () => {
  vi.spyOn(deviceKey, "deviceKeyFingerprint").mockResolvedValue("fingerprint")
  const claimed = {
    ...saved(),
    phase: "claimed" as const,
    claimedDeviceKey: Array(32).fill(7),
  }
  vi.mocked(scope.pairing.create).mockResolvedValue({
    code: "ABCD-EFGH",
    status: claimed,
  })
  vi.mocked(scope.pairing.approve).mockResolvedValueOnce({
    status: { ...claimed, phase: "active" },
    activationStopped: "retryable",
  })
  await mount()
  await click("Continue")
  await click("Create invitation")
  await click("Approve matching fingerprint")
  expect(container.querySelector("[data-gateway-fingerprint]")?.textContent).toBe(
    "fingerprint",
  )
  expect(container.textContent).toContain("Activation needs another attempt")
  expect(scope.close).not.toHaveBeenCalled()
  vi.mocked(scope.pairing.approve).mockResolvedValueOnce({
    status: { ...claimed, phase: "active" },
  })
  await click("Retry activation")
  expect(scope.pairing.approve).toHaveBeenNthCalledWith(
    1,
    claimed.invitationId,
    claimed.claimedDeviceKey,
  )
  expect(scope.pairing.approve).toHaveBeenNthCalledWith(
    2,
    claimed.invitationId,
    claimed.claimedDeviceKey,
  )
  expect(scope.close).toHaveBeenCalledOnce()
  expect(container.textContent).toContain("Reads still need a separate share")
})

it("withholds approval if the fingerprint cannot be computed", async () => {
  vi.spyOn(deviceKey, "deviceKeyFingerprint").mockRejectedValue(
    new Error("crypto unavailable"),
  )
  vi.mocked(scope.pairing.create).mockResolvedValue({
    code: "ABCD-EFGH",
    status: { ...saved(), phase: "claimed", claimedDeviceKey: Array(32).fill(7) },
  })
  await mount()
  await click("Continue")
  await click("Create invitation")
  expect(container.textContent).toContain("Fingerprint unavailable")
  const approve = present(
    [...container.querySelectorAll("button")].find(
      (each) => each.textContent === "Approve matching fingerprint",
    ),
  )
  expect(approve.disabled).toBe(true)
  expect(scope.pairing.approve).not.toHaveBeenCalled()
})

it("keeps worker and owner-phone pending invitations outside the PeerRead entry", async () => {
  vi.mocked(scope.pairing.pending).mockResolvedValue({
    items: [
      { ...saved(), class: "peer-gateway-environment-serve" },
      {
        ...saved(),
        invitationId: Array(16).fill(4),
        class: "gateway-owner-conversation-read-write",
        samePerson: undefined,
      },
    ],
  })
  await mount()
  await click("Resume panel invitations")
  expect(container.textContent).not.toContain("Resume 01")
  expect(container.textContent).not.toContain("Resume 04")
  expect(scope.pairing.create).not.toHaveBeenCalled()
})

it("shows an absent saved choice as unknown after recovery", async () => {
  vi.mocked(scope.pairing.pending).mockResolvedValue({
    items: [{ ...saved(), samePerson: undefined }],
  })
  await mount()
  await click("Resume panel invitations")
  const resume = present(
    [...container.querySelectorAll("button")].find((each) =>
      each.textContent?.includes("choice unknown — Waiting for the other gateway"),
    ),
  )
  await act(async () => resume.click())
  expect(container.querySelector("[data-gateway-choice]")?.textContent).toContain(
    "unknown",
  )
  expect(container.querySelector("[data-gateway-choice]")?.textContent).not.toContain(
    "no person link",
  )
  expect(container.querySelector("[data-gateway-code]")).toBeNull()
  expect(scope.pairing.create).not.toHaveBeenCalled()
})

it("discards an unsubmitted owner token on pagehide before the page can be cached", async () => {
  await mount()
  await chooseOwner()
  expect(
    present(container.querySelector<HTMLInputElement>('[type="password"]')).value,
  ).toBe("owner-secret")
  await act(async () => window.dispatchEvent(new Event("pagehide")))
  expect(
    present(container.querySelector<HTMLInputElement>('[type="password"]')).value,
  ).toBe("")
  expect(gateway.open).not.toHaveBeenCalled()
})
