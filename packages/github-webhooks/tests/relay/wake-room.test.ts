import { test } from "node:test"
import assert from "node:assert/strict"
import { WakeRoom, type RoomState } from "../../src/relay/infrastructure/wake-room.js"

test("expired wake sockets close and surviving leases schedule the next expiry", async () => {
  const effects: string[] = []
  const socket = (expiresAt: number) => ({
    deserializeAttachment: () => ({ expiresAt }),
    close: (code: number) => effects.push(`close:${expiresAt}:${code}`),
    send: (text: string) => effects.push(`send:${expiresAt}:${text}`),
  })
  const state = {
    getWebSockets: () => [socket(999), socket(2000)],
    acceptWebSocket: () => {},
    storage: {
      getAlarm: async () => null,
      setAlarm: async (at: number) => {
        effects.push(`alarm:${at}`)
      },
    },
  } as unknown as RoomState
  const room = new WakeRoom(state, () => 1000)
  await room.alarm()
  assert.deepEqual(effects, ["close:999:1000", "alarm:2000"])
  effects.length = 0
  await room.fetch(new Request("https://room/wake", { method: "POST" }))
  assert.deepEqual(effects, ["close:999:1000", 'send:2000:{"type":"changed"}'])
})

test("R19: an unavailable expiry alarm refuses before registering a socket", async () => {
  let accepted = false
  const state = {
    getWebSockets: () => [],
    acceptWebSocket: () => {
      accepted = true
    },
    storage: {
      getAlarm: async () => null,
      setAlarm: async () => {
        throw new Error("alarm unavailable")
      },
    },
  } as unknown as RoomState
  const room = new WakeRoom(state, () => 1000)
  await assert.rejects(
    room.fetch(
      new Request("https://room/watch", {
        headers: {
          Upgrade: "websocket",
          "x-relay-expires": "1100",
          "x-relay-user": "11",
        },
      }),
    ),
    /alarm unavailable/,
  )
  assert.equal(accepted, false)
})
