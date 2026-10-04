import { describe, expect, it } from "vitest"
import { parseWireMessage } from "./validate.js"
import { validChangeWatchId, watchResult } from "./change-watch-validate.js"

const watchId = "00000000-0000-4000-8000-000000000001-1"
function encoded(event: string, payload: unknown): string {
  return JSON.stringify({ type: "event", event, payload, seq: 1, stateVersion: 0 })
}

describe("payloadless change watch boundary", () => {
  it("accepts advisory identity and terminal source failure without progress", () => {
    expect(parseWireMessage(encoded("conversation.changed", { watchId }))).toMatchObject({
      payload: { watchId },
    })
    expect(
      parseWireMessage(
        encoded("conversation.watchEnded", { watchId, reason: "notification_failed" }),
      ),
    ).toMatchObject({ payload: { watchId, reason: "notification_failed" } })
  })

  it("rejects authority/progress smuggling and unknown terminal meanings", () => {
    for (const payload of [
      { watchId, head: "5" },
      { watchId, permission: true },
      { watchId: 1 },
      { watchId, cursor: "0" },
    ]) {
      expect(parseWireMessage(encoded("conversation.changed", payload))).toBeNull()
    }
    for (const reason of ["current", "revoked", "dirty", 1, null]) {
      expect(
        parseWireMessage(encoded("conversation.watchEnded", { watchId, reason })),
      ).toBeNull()
    }
  })

  it("refuses terminal fields that claim progress or authority beside a valid reason", () => {
    for (const reason of ["closed", "notification_failed"]) {
      for (const extra of [{ head: "5" }, { permission: true }, { cursor: "0" }]) {
        expect(
          parseWireMessage(
            encoded("conversation.watchEnded", { watchId, reason, ...extra }),
          ),
        ).toBeNull()
      }
    }
  })
  it("refuses a malformed namespace even when the counter is canonical", () => {
    for (const invalid of ["invalid-1", "registration-1", `😀-${1}`]) {
      expect(validChangeWatchId(invalid)).toBe(false)
      expect(
        parseWireMessage(encoded("conversation.changed", { watchId: invalid })),
      ).toBeNull()
    }
    expect(validChangeWatchId(watchId)).toBe(true)
  })
  it("rejects duplicate wire keys and noncanonical or overflowing counters", () => {
    expect(
      parseWireMessage(
        `{"type":"event","event":"conversation.changed","payload":{"watchId":"${watchId}","watchId":"${watchId}"},"seq":1,"stateVersion":0}`,
      ),
    ).toBeNull()
    const namespace = watchId.slice(0, watchId.lastIndexOf("-"))
    for (const counter of ["0", "01", "18446744073709551616", "-1", "1e2"]) {
      expect(validChangeWatchId(`${namespace}-${counter}`)).toBe(false)
    }
    expect(validChangeWatchId(`${namespace}-18446744073709551615`)).toBe(true)
    expect(() => watchResult(Object.create({ watchId }))).toThrow(
      "Invalid change watch response",
    )
  })
})
