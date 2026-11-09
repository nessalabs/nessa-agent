import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  linkDeliveryMs,
  summarizeViews,
  throttleProfile,
} from "./subscription-byte-summary.mjs"

describe("subscription byte summary", () => {
  it("totals frames and compares them with the turn's own text", () => {
    const summary = summarizeViews(
      [
        { bytes: 1000, payloadBytes: 900 },
        { bytes: 2500, payloadBytes: 2400 },
      ],
      500,
    )
    assert.deepEqual(summary, {
      frameCount: 2,
      totalBytes: 3500,
      maxFrameBytes: 2500,
      maxPayloadBytes: 2400,
      framesNearBound: 0,
      agentTextBytes: 500,
      totalBytesPerTextByte: 7,
    })
  })

  it("counts a payload within a tenth of the frame bound as near it", () => {
    const summary = summarizeViews(
      [{ bytes: 70_000, payloadBytes: throttleProfile.frameBoundBytes - 100 }],
      0,
    )
    assert.equal(summary.framesNearBound, 1)
    assert.equal(summary.totalBytesPerTextByte, null)
    assert.equal(throttleProfile.rttMs, 150)
    assert.equal(throttleProfile.downBytesPerSecond, 200_000)
  })

  it("overlaps one-way delay and serializes only the downlink", () => {
    const down = Array.from({ length: 10 }, () => ({ direction: "down", bytes: 100 }))
    const planned = linkDeliveryMs([...down, { direction: "up", bytes: 1000 }], {
      propagationMs: 75,
      downBytesPerSecond: 200_000,
      nowMs: 0,
    })
    assert.equal(planned.at[0], 75.5)
    assert.equal(planned.at[9], 80)
    assert.equal(planned.at[10], 75)
    assert.equal(planned.downFreeAt, 5)
  })

  it("reports an empty turn as no frames", () => {
    assert.deepEqual(summarizeViews([], 10), {
      frameCount: 0,
      totalBytes: 0,
      maxFrameBytes: 0,
      maxPayloadBytes: 0,
      framesNearBound: 0,
      agentTextBytes: 10,
      totalBytesPerTextByte: 0,
    })
  })
})
