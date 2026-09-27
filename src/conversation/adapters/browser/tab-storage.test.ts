import { expect, it } from "vitest"
import { createTabStorage } from "./tab-storage"
it("restores only the authenticated owner and gateway's references", () => {
  const values = new Map<string, string>()
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value)
    },
  }
  const owner = { gatewayId: "gateway", principalId: "alice", organizationId: "org" }
  const conversationId = "00000000-0000-4000-8000-000000000001"
  createTabStorage(storage).write(owner, {
    tabs: [{ conversationId, title: "My chat" }],
    activeConversationId: conversationId,
  })
  expect(createTabStorage(storage).read(owner)?.tabs[0]?.conversationId).toBe(
    conversationId,
  )
  expect(createTabStorage(storage).read({ ...owner, principalId: "bob" })).toBeNull()
  expect(createTabStorage(storage).read({ ...owner, gatewayId: "other" })).toBeNull()
})
it("rejects saved references that the gateway cannot address as canonical UUIDs", () => {
  const owner = { gatewayId: "g", principalId: "p", organizationId: "o" }
  const invalid = [
    "chat",
    "00000000-0000-4000-8000-00000000000A",
    "{00000000-0000-4000-8000-000000000001}",
  ]
  for (const conversationId of invalid) {
    const storage = new Map<string, string>()
    const browser = {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => {
        storage.set(key, value)
      },
    }
    createTabStorage(browser).write(owner, { tabs: [{ conversationId }] })
    expect(createTabStorage(browser).read(owner)).toBeNull()
  }
})
it("ignores corrupt or unavailable browser storage without interrupting sign-in", () => {
  const owner = { gatewayId: "g", principalId: "p", organizationId: "o" }
  expect(
    createTabStorage({ getItem: () => "null", setItem: () => {} }).read(owner),
  ).toBeNull()
  expect(
    createTabStorage({
      getItem: () => {
        throw new Error()
      },
      setItem: () => {},
    }).read(owner),
  ).toBeNull()
})
