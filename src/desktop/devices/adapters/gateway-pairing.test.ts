import { expect, it, vi } from "vitest"
import { NessaClient, type PairingApi } from "@nessa/client"
import { gatewayInvitation, gatewayPairing } from "./gateway-pairing"

const identity = { gatewayId: "gateway", organizationId: "org", audienceId: "gateway" }
function fixture() {
  const pairing = {
    pending: vi.fn(async () => ({ items: [] })),
    create: vi.fn(),
    status: vi.fn(),
    approve: vi.fn(),
    cancel: vi.fn(),
  } as unknown as PairingApi
  const panel = {
    auth: { session: vi.fn(async () => identity) },
    pairing,
    sessionUrl: "ws://127.0.0.1:7421/browser/session" as string | undefined,
  }
  const privileged = {
    auth: { session: vi.fn(async () => identity) },
    pairing,
    close: vi.fn(),
  }
  const connect = vi.fn<typeof NessaClient.connect>(
    async () => privileged as unknown as NessaClient,
  )
  const connected = vi.fn(async () => panel as never)
  const gateway = gatewayPairing({
    connected,
    stage: "ci",
    connect,
    newId: () => "pairing-scope",
    after: () => () => {},
  })
  return { gateway, panel, privileged, pairing, connect, connected }
}

it("false uses the ordinary panel and closing the scope cannot close that client", async () => {
  const f = fixture()
  const abort = new AbortController()
  const scope = await f.gateway.open(undefined, abort.signal)
  await scope.pairing.pending()
  expect(f.connect).not.toHaveBeenCalled()
  scope.close()
  await expect(scope.pairing.pending()).rejects.toMatchObject({ name: "AbortError" })
  expect(f.pairing.pending).toHaveBeenCalledTimes(1)
})

it("pins the current endpoint, isolates owner authentication, and disables recovery", async () => {
  const f = fixture()
  const scope = await f.gateway.open("owner-secret", new AbortController().signal)
  const options = f.connect.mock.calls[0]![0]
  expect(options).toMatchObject({
    url: "ws://127.0.0.1:7421/session",
    auth: { credential: "owner-secret" },
    config: { retry: { maxAttempts: 1 }, reconnect: { enabled: false } },
  })
  expect(f.panel.auth.session).toHaveBeenCalledOnce()
  expect(f.privileged.auth.session).toHaveBeenCalledOnce()
  scope.close()
  expect(f.privileged.close).toHaveBeenCalledOnce()
  await expect(scope.pairing.create(gatewayInvitation(true))).rejects.toMatchObject({
    name: "AbortError",
  })
  expect(f.pairing.create).not.toHaveBeenCalled()
})

it.each(["gatewayId", "organizationId", "audienceId"] as const)(
  "refuses mismatched %s before a command",
  async (field) => {
    const f = fixture()
    f.privileged.auth.session.mockResolvedValue({ ...identity, [field]: "wrong" })
    await expect(
      f.gateway.open("owner-secret", new AbortController().signal),
    ).rejects.toThrow("different gateway")
    expect(f.privileged.close).toHaveBeenCalledOnce()
    expect(f.pairing.create).not.toHaveBeenCalled()
  },
)

it("closes a connection which finishes after cancellation without exposing commands", async () => {
  const f = fixture()
  let finish!: (client: NessaClient) => void
  f.connect.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const abort = new AbortController()
  const opened = f.gateway.open("owner-secret", abort.signal)
  await vi.waitFor(() => expect(f.connect).toHaveBeenCalledOnce())
  abort.abort()
  finish(f.privileged as unknown as NessaClient)
  await expect(opened).rejects.toMatchObject({ name: "AbortError" })
  expect(f.privileged.auth.session).not.toHaveBeenCalled()
  expect(f.privileged.close).toHaveBeenCalledOnce()
  expect(f.pairing.create).not.toHaveBeenCalled()
})

it("keeps an unconfirmed mutation un-replayed and supports canonical recovery", async () => {
  const f = fixture()
  f.pairing.create = vi.fn().mockRejectedValue(new Error("lost reply"))
  const scope = await f.gateway.open(undefined, new AbortController().signal)
  await expect(scope.pairing.create(gatewayInvitation(false))).rejects.toThrow(
    "lost reply",
  )
  await scope.pairing.pending()
  expect(f.pairing.create).toHaveBeenCalledExactlyOnceWith({
    enrollee: "gateway",
    samePerson: false,
  })
  expect(f.pairing.pending).toHaveBeenCalledOnce()
  scope.close()
})

it("does not authenticate owner evidence without a connected endpoint", async () => {
  const f = fixture()
  f.panel.sessionUrl = undefined
  await expect(
    f.gateway.open("owner-secret", new AbortController().signal),
  ).rejects.toThrow("reconnecting")
  expect(f.connect).not.toHaveBeenCalled()
  expect(f.pairing.create).not.toHaveBeenCalled()
})

it("refuses an already aborted scope before reading the panel", async () => {
  const f = fixture()
  const abort = new AbortController()
  abort.abort()
  await expect(f.gateway.open("owner-secret", abort.signal)).rejects.toMatchObject({
    name: "AbortError",
  })
  expect(f.connected).not.toHaveBeenCalled()
  expect(f.panel.auth.session).not.toHaveBeenCalled()
  expect(f.connect).not.toHaveBeenCalled()
})

it("refuses a primary reconnect between endpoint and identity reads before sending owner evidence", async () => {
  const f = fixture()
  let finish!: (value: typeof identity) => void
  f.panel.auth.session.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const opened = f.gateway.open("owner-secret", new AbortController().signal)
  await vi.waitFor(() => expect(f.panel.auth.session).toHaveBeenCalledOnce())
  f.panel.sessionUrl = "ws://127.0.0.1:7444/session"
  finish(identity)
  await expect(opened).rejects.toThrow("changed during sign-in")
  expect(f.connect).not.toHaveBeenCalled()
  expect(f.pairing.create).not.toHaveBeenCalled()
})

it("preserves the trusted product route and routing query rather than selecting another endpoint", async () => {
  const f = fixture()
  f.panel.sessionUrl = "ws://127.0.0.1:7421/team/session?route=tenant"
  const scope = await f.gateway.open("owner-secret", new AbortController().signal)
  expect(f.connect).toHaveBeenCalledWith(
    expect.objectContaining({ url: f.panel.sessionUrl }),
  )
  scope.close()
})

it.each(["primary", "panel identity", "owner connect", "owner identity"])(
  "abandons pending %s promptly on abort without closing the shared panel",
  async (wait) => {
    const f = fixture()
    if (wait === "primary") f.connected.mockReturnValue(new Promise(() => {}))
    if (wait === "panel identity")
      f.panel.auth.session.mockReturnValue(new Promise(() => {}))
    if (wait === "owner connect") f.connect.mockReturnValue(new Promise(() => {}))
    if (wait === "owner identity")
      f.privileged.auth.session.mockReturnValue(new Promise(() => {}))
    const abort = new AbortController()
    const opened = f.gateway.open("owner-secret", abort.signal)
    const outcome = opened.catch((error: unknown) => error)
    const waiting =
      wait === "primary"
        ? f.connected
        : wait === "panel identity"
          ? f.panel.auth.session
          : wait === "owner connect"
            ? f.connect
            : f.privileged.auth.session
    await vi.waitFor(() => expect(waiting).toHaveBeenCalledOnce())
    abort.abort()
    const answer = await Promise.race([
      outcome,
      new Promise((resolve) => setTimeout(() => resolve({ name: "still-pending" }), 0)),
    ])
    expect(answer).toMatchObject({ name: "AbortError" })
    if (wait !== "owner identity") expect(f.privileged.close).not.toHaveBeenCalled()
    else expect(f.privileged.close).toHaveBeenCalledOnce()
    if (wait === "primary" || wait === "panel identity")
      expect(f.connect).not.toHaveBeenCalled()
  },
)

it("retires an owned result when abort lands between promise resolution and the awaiting continuation", async () => {
  const f = fixture()
  let finish!: (value: NessaClient) => void
  f.connect.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const abort = new AbortController()
  const opened = f.gateway.open("owner-secret", abort.signal)
  const outcome = opened.catch((error: unknown) => error)
  await vi.waitFor(() => expect(f.connect).toHaveBeenCalledOnce())
  finish(f.privileged as unknown as NessaClient)
  queueMicrotask(() => abort.abort())
  expect(await outcome).toMatchObject({ name: "AbortError" })
  expect(f.privileged.close).toHaveBeenCalledOnce()
  expect(f.privileged.auth.session).not.toHaveBeenCalled()
  expect(f.pairing.create).not.toHaveBeenCalled()
})
