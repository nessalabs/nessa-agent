import { describe, expect, it, vi } from "vitest"

import { createPeerApi } from "./peer-api.js"

const peerKey = Array.from({ length: 32 }, (_, index) => index + 1)

function session(answer: unknown) {
  const calls: { method: string; params: unknown }[] = []
  const request = vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params })
    return answer
  })
  return { api: createPeerApi({ request }), calls }
}

describe("client.peer", () => {
  it("enrolls with the address and the code", async () => {
    const { api, calls } = session({ peerKey, phase: "pending" })
    const enrolled = await api.enroll({ address: "127.0.0.1:47650", code: "ABCD-2345" })
    expect(calls).toEqual([
      {
        method: "peer.enroll",
        params: { address: "127.0.0.1:47650", code: "ABCD-2345" },
      },
    ])
    expect(enrolled.phase).toBe("pending")
  })

  it("stores a follow rule and lists the peers", async () => {
    const followed = {
      peerKey,
      conversations: ["00000000-0000-4000-8000-000000000890"],
      keepBytes: 268435456,
      replicaReset: true,
    }
    const { api, calls } = session(followed)
    const result = await api.follow({
      peerKey,
      conversations: followed.conversations,
      keepBytes: followed.keepBytes,
    })
    expect(calls).toEqual([
      {
        method: "peer.follow",
        params: {
          peerKey,
          conversations: followed.conversations,
          keepBytes: followed.keepBytes,
        },
      },
    ])
    expect(result.replicaReset).toBe(true)
    calls.length = 0
    const listed = session({
      items: [{ peerKey, phase: "active", credentialId: "credential" }],
    })
    expect((await listed.api.list()).items[0]?.credentialId).toBe("credential")
    expect(listed.calls).toEqual([{ method: "peer.list", params: {} }])
  })
})
