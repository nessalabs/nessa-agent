import assert from "node:assert/strict"
import { test } from "node:test"
import { receive, type StoreResult } from "./receive.js"
import type { Delivery } from "../domain/delivery.js"

const delivery: Delivery = {
  identity: { id: "aaaaaaaa-1111-2222-3333-444444444444", event: "issues" },
  payload: "{}",
  payloadSha256: "digest",
  receivedAt: "2026-10-25T00:00:00Z",
  bytes: 2,
}

test("the injected persistence owner decides acceptance", async () => {
  for (const result of [
    "stored",
    "duplicate",
    "conflict",
    "full",
  ] satisfies StoreResult[]) {
    assert.equal(await receive(delivery, { retain: async () => result }), result)
  }
})

test("lost persistence answers remain unconfirmed", async () => {
  assert.equal(
    await receive(delivery, {
      retain: async () => {
        throw new Error("offline")
      },
    }),
    "unavailable",
  )
})

test("acceptance waits for the durable receipt", async () => {
  let finish!: (value: StoreResult) => void
  const pending = new Promise<StoreResult>((resolve) => {
    finish = resolve
  })
  let settled = false
  const result = receive(delivery, { retain: () => pending }).then((value) => {
    settled = true
    return value
  })
  await Promise.resolve()
  assert.equal(settled, false)
  finish("stored")
  assert.equal(await result, "stored")
})
