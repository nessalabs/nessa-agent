import { describe, expect, it, vi } from "vitest"
import { NessaConnectionClosedError, NessaRpcError } from "@nessa/client"
import { LOCAL_GATEWAY_ID, type PersistedRemoteGateway } from "../../model/gateway-id"
import {
  inMemoryGatewayCredentialStore,
  type RemoteGatewayStore,
} from "../storage/gateway-store"
import {
  CLOUD_BOX_CREATE_ATTEMPTS,
  createGatewayRegistry,
  type RemoteClient,
  type RemoteClientConnector,
} from "./gateway-registry-adapter"

function memoryStore(initial: readonly PersistedRemoteGateway[] = []): {
  store: RemoteGatewayStore
  current: () => readonly PersistedRemoteGateway[]
} {
  let saved = [...initial]
  return {
    store: {
      read: () => saved,
      write: (next) => {
        saved = [...next]
      },
    },
    current: () => saved,
  }
}

function fakeRemoteClient(gatewayId: string): {
  client: RemoteClient
  closed: ReturnType<typeof vi.fn>
  emitState: (state: { status: "connected" } | { status: "closed"; error: Error }) => void
} {
  const closed = vi.fn()
  const stateListeners = new Set<
    (state: { status: "connected" } | { status: "closed"; error: Error }) => void
  >()
  const client = {
    productSession: { gatewayId },
    connectionState: { status: "connected" },
    onConnectionStateChange: (listener: (state: never) => void) => {
      stateListeners.add(listener as never)
      return () => stateListeners.delete(listener as never)
    },
    on: () => () => {},
    onClose: () => () => {},
    close: closed,
  } as unknown as RemoteClient
  return {
    client,
    closed,
    emitState: (state) => {
      for (const l of stateListeners) l(state)
    },
  }
}

describe("createGatewayRegistry (ADR 1417 G3, G4, G7, G8, G9, G11)", () => {
  const localStub = {
    label: "This Mac",
    url: "local",
    workspace: {} as never,
    terminals: {} as never,
    channels: {} as never,
  }

  it("verifies handshake before persisting, keeps credentials out of metadata store, and deduplicates by stable gatewayId (G7, B4, M6)", async () => {
    const { store, current } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const first = fakeRemoteClient("gw-cloud-1")
    const second = fakeRemoteClient("gw-cloud-1")
    let call = 0
    const connectRemote: RemoteClientConnector = vi.fn(async () => {
      call += 1
      return call === 1 ? first.client : second.client
    })

    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 1000,
      after: () => () => {},
      connectRemote,
    })

    const added = await registry.add({
      label: "Cloud Box",
      url: "wss://box1.example.com",
      credential: "tok-1",
    })
    expect(added.gatewayId).toBe("gw-cloud-1")
    expect(added.status).toBe("connected")
    expect(current()).toHaveLength(1)
    expect("credential" in current()[0]).toBe(false)
    expect(credentialStore.read("gw-cloud-1")).toBe("tok-1")

    // Re-pairing updates URL, label, and credential in place without duplicating.
    const repaired = await registry.add({
      label: "Cloud Box v2",
      url: "wss://box1-rotated.example.com",
      credential: "tok-2",
    })
    expect(repaired.gatewayId).toBe("gw-cloud-1")
    expect(repaired.label).toBe("Cloud Box v2")
    expect(first.closed).toHaveBeenCalledTimes(1)
    expect(current()).toHaveLength(1)
    expect(credentialStore.read("gw-cloud-1")).toBe("tok-2")
    registry.dispose()
  })

  it("refuses cleartext remote ws://, missing/local gatewayId, and auth failures without persisting (G7, G9, M5, M6)", async () => {
    const { store, current } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const connectRemote: RemoteClientConnector = vi.fn(async ({ url }) => {
      if (url.includes("revoked")) {
        throw new NessaRpcError("unauthorized", "token revoked")
      }
      if (url.includes("local-spoof")) {
        return fakeRemoteClient(LOCAL_GATEWAY_ID).client
      }
      return fakeRemoteClient("").client
    })

    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 1000,
      after: () => () => {},
      connectRemote,
    })

    await expect(
      registry.add({ label: "Bad", url: "ws://remote.example.com", credential: "tok" }),
    ).rejects.toMatchObject({ reason: "invalid-url" })

    await expect(
      registry.add({
        label: "Revoked",
        url: "wss://revoked.example.com",
        credential: "bad-tok",
      }),
    ).rejects.toMatchObject({ reason: "auth-refused" })

    await expect(
      registry.add({
        label: "Spoof",
        url: "wss://local-spoof.example.com",
        credential: "tok",
      }),
    ).rejects.toMatchObject({ reason: "connect-failed" })

    await expect(
      registry.add({ label: "NoId", url: "wss://noid.example.com", credential: "tok" }),
    ).rejects.toMatchObject({ reason: "connect-failed" })

    expect(current()).toHaveLength(0)
    expect(registry.list()).toHaveLength(1)
    registry.dispose()
  })

  it("transitions to auth-refused on live credential_expired close and refuses silent reconnect (G9, M2, M3)", async () => {
    const { store } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const remote = fakeRemoteClient("gw-live")
    const connectRemote: RemoteClientConnector = vi.fn(async () => remote.client)

    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 1000,
      after: () => () => {},
      connectRemote,
    })

    await registry.add({
      label: "Live Box",
      url: "wss://live.example.com",
      credential: "tok",
    })
    expect(connectRemote).toHaveBeenCalledTimes(1)

    remote.emitState({
      status: "closed",
      error: new NessaConnectionClosedError(4003, ""),
    })
    expect(registry.list().find((g) => g.gatewayId === "gw-live")?.status).toBe(
      "auth-refused",
    )

    // reconnect() and wake() must both refuse when auth-refused (G9, R2-M2).
    registry.reconnect("gw-live")
    expect(connectRemote).toHaveBeenCalledTimes(1)
    await expect(registry.wake!("gw-live")).rejects.toMatchObject({
      reason: "auth-refused",
    })
    registry.dispose()
  })

  it("restores persisted remote without credential as auth-refused with zero reconnects (R2-M1)", () => {
    const { store } = memoryStore([
      {
        gatewayId: "gw-restored",
        label: "Restored Box",
        url: "wss://restored.example.com/",
        addedAtMs: 500,
      },
    ])
    const connectRemote = vi.fn()
    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore: inMemoryGatewayCredentialStore(),
      now: () => 1000,
      after: () => () => {},
      connectRemote,
    })

    const restored = registry.list().find((g) => g.gatewayId === "gw-restored")
    expect(restored?.status).toBe("auth-refused")
    expect(connectRemote).not.toHaveBeenCalled()
    registry.dispose()
  })

  it("updates URL and reconnects on updateEndpoint (G8) and removes cleanly on remove() (G11)", async () => {
    const { store, current } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const first = fakeRemoteClient("gw-devbox")
    const second = fakeRemoteClient("gw-devbox")
    let calls = 0
    const connectRemote: RemoteClientConnector = vi.fn(async () => {
      calls += 1
      return calls === 1 ? first.client : second.client
    })
    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 1000,
      after: () => () => {},
      connectRemote,
    })

    await registry.add({
      label: "Devbox",
      url: "wss://devbox-1.example.com",
      credential: "tok",
    })
    registry.updateEndpoint!("gw-devbox", "wss://devbox-2.example.com")
    expect(current()[0].url).toBe("wss://devbox-2.example.com/")
    expect(first.closed).toHaveBeenCalledTimes(1)

    await expect(registry.remove(LOCAL_GATEWAY_ID)).rejects.toMatchObject({
      reason: "cannot-remove-local",
    })

    await registry.remove("gw-devbox")
    expect(current()).toHaveLength(0)
    expect(credentialStore.read("gw-devbox")).toBeNull()
    expect(registry.list()).toHaveLength(1)
    registry.dispose()
  })

  it("rejects and closes a reconnected endpoint reporting a different gatewayId", async () => {
    const { store } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const original = fakeRemoteClient("gw-original")
    const imposter = fakeRemoteClient("gw-other-server")
    let calls = 0
    const connectRemote: RemoteClientConnector = vi.fn(async () => {
      calls += 1
      return calls === 1 ? original.client : imposter.client
    })
    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 1000,
      after: () => () => {},
      connectRemote,
    })

    await registry.add({
      label: "Box",
      url: "wss://box-1.example.com",
      credential: "tok",
    })
    registry.updateEndpoint!("gw-original", "wss://box-reassigned.example.com")
    // Allow reconnect promise to settle.
    await Promise.resolve()
    await Promise.resolve()
    expect(imposter.closed).toHaveBeenCalledTimes(1)
    expect(registry.list().find((g) => g.gatewayId === "gw-original")?.status).toBe(
      "refused",
    )
    registry.dispose()
  })

  it("coalesces concurrent wake() calls (G3) and marks refused on wake error (G4)", async () => {
    const { store } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const remote = fakeRemoteClient("gw-sleepy")
    let resolveWake!: () => void
    let rejectWake!: (err: Error) => void
    let mode: "ok" | "fail" = "ok"
    const wakeGateway = vi.fn(
      () =>
        new Promise<void>((resolve, reject) => {
          if (mode === "ok") resolveWake = resolve
          else rejectWake = reject
        }),
    )

    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 1000,
      after: () => () => {},
      connectRemote: async () => remote.client,
      wakeGateway,
    })

    await registry.add({
      label: "Sleepy Box",
      url: "wss://sleepy.example.com",
      credential: "tok",
    })

    const p1 = registry.wake!("gw-sleepy")
    const p2 = registry.wake!("gw-sleepy")
    expect(wakeGateway).toHaveBeenCalledTimes(1)
    expect(registry.list().find((g) => g.gatewayId === "gw-sleepy")?.status).toBe(
      "waking",
    )
    resolveWake()
    await Promise.all([p1, p2])

    // G4: wake refusal marks gateway refused with detail.
    mode = "fail"
    const p3 = registry.wake!("gw-sleepy")
    rejectWake(new Error("quota exceeded"))
    await expect(p3).rejects.toThrow("quota exceeded")
    const after = registry.list().find((g) => g.gatewayId === "gw-sleepy")
    expect(after?.status).toBe("refused")
    expect(after?.detail).toBe("quota exceeded")
    registry.dispose()
  })

  it("creates, sleeps, wakes with URL rotation (G8), and deletes a Cloud Box gateway (#1420)", async () => {
    const { store, current } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const remote1 = fakeRemoteClient("gw-cloud-1")
    const remote2 = fakeRemoteClient("gw-cloud-1")
    let calls = 0
    const connectRemote: RemoteClientConnector = vi.fn(async () => {
      calls += 1
      return calls === 1 ? remote1.client : remote2.client
    })
    const cloudBoxes = {
      create: vi.fn(
        async ({
          boxId,
          name,
          cloud,
        }: {
          boxId: string
          name: string
          cloud: "boat" | "agent37"
        }) => ({
          boxId,
          name,
          cloud,
          url: "wss://box-1.example.com",
          credential: "enroll-secret",
        }),
      ),
      wake: vi.fn(async () => ({
        url: "wss://box-1-rotated.example.com",
      })),
      sleep: vi.fn(async () => {}),
      delete: vi.fn(async () => {}),
    }

    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 2000,
      after: () => () => {},
      connectRemote,
      cloudBoxes,
      newId: () => "11111111-2222-3333-4444-555555555555",
    })

    const created = await registry.createCloudBox!({
      name: "Cloud Box",
      cloud: "agent37",
    })
    // The gateway's id is the one its own handshake reported.
    expect(created.gatewayId).toBe("gw-cloud-1")
    expect(cloudBoxes.create).toHaveBeenCalledWith({
      boxId: "11111111-2222-3333-4444-555555555555",
      name: "Cloud Box",
      cloud: "agent37",
    })
    expect(created.cloudBox).toEqual({
      boxId: "11111111-2222-3333-4444-555555555555",
      cloud: "agent37",
    })
    expect(credentialStore.read("gw-cloud-1")).toBe("enroll-secret")
    expect(current()[0]?.cloudBox?.boxId).toBe("11111111-2222-3333-4444-555555555555")

    await registry.sleepCloudBox!("gw-cloud-1")
    expect(cloudBoxes.sleep).toHaveBeenCalledWith("11111111-2222-3333-4444-555555555555")
    expect(registry.list().find((g) => g.gatewayId === "gw-cloud-1")?.status).toBe(
      "disconnected",
    )

    // G8: wake refreshes rotated URL and reconnects.
    await registry.wake!("gw-cloud-1")
    expect(cloudBoxes.wake).toHaveBeenCalledWith("11111111-2222-3333-4444-555555555555")
    expect(current()[0]?.url).toBe("wss://box-1-rotated.example.com/")

    // G11: deleteCloudBox destroys VM and removes gateway.
    await registry.deleteCloudBox!("gw-cloud-1")
    expect(cloudBoxes.delete).toHaveBeenCalledWith("11111111-2222-3333-4444-555555555555")
    expect(registry.list().map((g) => g.gatewayId)).toEqual(["local"])
    expect(credentialStore.read("gw-cloud-1")).toBeNull()
    registry.dispose()
  })

  it("asks again for the same box after a lost answer or a failed enrollment, refuses a bad wake URL and sleep-while-waking, and cleans up a mid-dispose delete", async () => {
    const { store, current } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const genuine = fakeRemoteClient("gw-box-2")
    let connectCount = 0
    const connectRemote: RemoteClientConnector = vi.fn(async () => {
      connectCount += 1
      if (connectCount === 1) throw new Error("socket refused")
      return genuine.client
    })
    let resolveWake!: (res: { url: string }) => void
    let resolveDelete!: () => void
    let createCount = 0
    const cloudBoxes = {
      create: vi.fn(async ({ boxId }: { boxId: string }) => {
        createCount += 1
        // The first answer is lost (a timeout carries no refusal).
        if (createCount === 1) throw new Error("request timed out")
        return {
          boxId,
          name: "Box 2",
          cloud: "agent37" as const,
          url: "wss://box-2.example.com",
          credential: `tok-${createCount}`,
        }
      }),
      wake: vi.fn(
        () =>
          new Promise<{ url: string }>((resolve) => {
            resolveWake = resolve
          }),
      ),
      sleep: vi.fn(async () => {}),
      delete: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            resolveDelete = resolve
          }),
      ),
    }

    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 3000,
      after: () => () => {},
      connectRemote,
      cloudBoxes,
      newId: () => "22222222-3333-4444-5555-666666666666",
    })

    // A lost answer, then an enrollment that cannot connect, then success:
    // every attempt names the same box, so no second VM is made or deleted.
    const created = await registry.createCloudBox!({ name: "Box 2", cloud: "agent37" })
    expect(created.gatewayId).toBe("gw-box-2")
    expect(cloudBoxes.create).toHaveBeenCalledTimes(3)
    for (const [input] of cloudBoxes.create.mock.calls) {
      expect(input.boxId).toBe("22222222-3333-4444-5555-666666666666")
    }
    expect(cloudBoxes.delete).not.toHaveBeenCalled()
    expect(credentialStore.read("gw-box-2")).toBe("tok-3")

    // sleepCloudBox refuses while wake is in flight, and an invalid wake URL rejects.
    const wakePromise = registry.wake!("gw-box-2")
    await expect(registry.sleepCloudBox!("gw-box-2")).rejects.toMatchObject({
      reason: "offline",
    })
    resolveWake({ url: "ws://insecure.example.com" })
    await expect(wakePromise).rejects.toMatchObject({ reason: "invalid-url" })

    // dispose() mid-delete still cleans credentialStore and persisted store.
    const deletePromise = registry.deleteCloudBox!("gw-box-2")
    registry.dispose()
    resolveDelete()
    await deletePromise
    expect(credentialStore.read("gw-box-2")).toBeNull()
    expect(current()).toEqual([])
  })

  it("does not ask again after a refusal a retry cannot change", async () => {
    const { store, current } = memoryStore()
    const cloudBoxes = {
      create: vi.fn(async () => {
        throw Object.assign(new Error("Cloud box refused"), {
          refusal: "cloud_box_admission_refused",
        })
      }),
      wake: vi.fn(async () => ({ url: "wss://unused.example.com" })),
      sleep: vi.fn(async () => {}),
      delete: vi.fn(async () => {}),
    }
    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore: inMemoryGatewayCredentialStore(),
      now: () => 3500,
      after: () => () => {},
      connectRemote: async () => {
        throw new Error("never reached")
      },
      cloudBoxes,
    })

    await expect(
      registry.createCloudBox!({ name: "Refused Box", cloud: "agent37" }),
    ).rejects.toMatchObject({ refusal: "cloud_box_admission_refused" })
    expect(cloudBoxes.create).toHaveBeenCalledTimes(1)
    expect(cloudBoxes.delete).not.toHaveBeenCalled()
    expect(current()).toEqual([])
    registry.dispose()
  })

  it("asks again for the same box after a provider failure, which leaves the creation resumable", async () => {
    const { store } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const genuine = fakeRemoteClient("gw-box-4")
    let createCount = 0
    const cloudBoxes = {
      create: vi.fn(async ({ boxId }: { boxId: string }) => {
        createCount += 1
        if (createCount === 1) {
          throw Object.assign(new Error("Cloud box provider failed"), {
            refusal: "cloud_box_provider_failed",
          })
        }
        return {
          boxId,
          name: "Retry Box",
          cloud: "agent37" as const,
          url: "wss://box-4.example.com",
          credential: "tok-4",
        }
      }),
      wake: vi.fn(async () => ({ url: "wss://unused.example.com" })),
      sleep: vi.fn(async () => {}),
      delete: vi.fn(async () => {}),
    }
    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 3700,
      after: () => () => {},
      connectRemote: async () => genuine.client,
      cloudBoxes,
      newId: () => "44444444-3333-4444-5555-666666666666",
    })

    const created = await registry.createCloudBox!({
      name: "Retry Box",
      cloud: "agent37",
    })
    expect(created.gatewayId).toBe("gw-box-4")
    expect(cloudBoxes.create).toHaveBeenCalledTimes(2)
    for (const [input] of cloudBoxes.create.mock.calls) {
      expect(input.boxId).toBe("44444444-3333-4444-5555-666666666666")
    }
    expect(cloudBoxes.delete).not.toHaveBeenCalled()
    registry.dispose()
  })

  it("deletes a box it still cannot enroll after every attempt, and names it when that delete fails", async () => {
    const { store, current } = memoryStore()
    const credentialStore = inMemoryGatewayCredentialStore()
    const cloudBoxes = {
      create: vi.fn(async ({ boxId }: { boxId: string }) => ({
        boxId,
        name: "Unreachable Box",
        cloud: "agent37" as const,
        url: "wss://box-3.example.com",
        credential: "tok-3",
      })),
      wake: vi.fn(async () => ({ url: "wss://box-3.example.com" })),
      sleep: vi.fn(async () => {}),
      delete: vi.fn(async () => {
        throw new Error("provider unavailable")
      }),
    }
    const registry = createGatewayRegistry({
      local: localStub,
      store,
      credentialStore,
      now: () => 4000,
      after: () => () => {},
      connectRemote: async () => {
        throw new Error("socket refused")
      },
      cloudBoxes,
      newId: () => "33333333-4444-5555-6666-777777777777",
    })

    await expect(
      registry.createCloudBox!({ name: "Unreachable Box", cloud: "agent37" }),
    ).rejects.toThrow("33333333-4444-5555-6666-777777777777")
    expect(cloudBoxes.create).toHaveBeenCalledTimes(CLOUD_BOX_CREATE_ATTEMPTS)
    expect(cloudBoxes.delete).toHaveBeenCalledWith("33333333-4444-5555-6666-777777777777")
    expect(current()).toEqual([])
    registry.dispose()
  })
})
