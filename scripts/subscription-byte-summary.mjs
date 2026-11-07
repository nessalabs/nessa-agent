/**
 * Turn totals for subscription frames already counted on the wire.
 * `frameBoundBytes` is `product.max_payload_bytes` (docs/limits.md): 65536.
 * A payload within a tenth of that bound counts as near it.
 */
export const throttleProfile = {
  rttMs: 150,
  downBitsPerSecond: 1_600_000,
  downBytesPerSecond: 200_000,
  frameBoundBytes: 65536,
}

/**
 * When each message is fully delivered on a link whose one-way delay is
 * `propagationMs` and whose downlink serializes at `downBytesPerSecond`.
 *
 * Serialization of downlink bytes is queued. Propagation starts when that
 * message's own bytes are on the wire, so it overlaps the next message's
 * serialization. Uplink messages wait only for propagation; they do not
 * queue behind the downlink. `nowMs` is when this batch is offered.
 *
 * Returns one delivery time per message, in the same order.
 */
export function linkDeliveryMs(
  messages,
  { propagationMs, downBytesPerSecond, nowMs = 0, downFreeAt = nowMs },
) {
  let freeAt = downFreeAt
  const at = messages.map((message) => {
    if (message.direction === "up") return nowMs + propagationMs
    const transmitMs = (message.bytes / downBytesPerSecond) * 1000
    const started = Math.max(nowMs, freeAt)
    freeAt = started + transmitMs
    return freeAt + propagationMs
  })
  return { at, downFreeAt: freeAt }
}

export function summarizeViews(frames, agentTextBytes) {
  const totalBytes = frames.reduce((sum, frame) => sum + frame.bytes, 0)
  const maxFrameBytes = frames.reduce((max, frame) => Math.max(max, frame.bytes), 0)
  const maxPayloadBytes = frames.reduce(
    (max, frame) => Math.max(max, frame.payloadBytes),
    0,
  )
  const nearBound = frames.filter(
    (frame) => frame.payloadBytes >= throttleProfile.frameBoundBytes * 0.9,
  ).length
  return {
    frameCount: frames.length,
    totalBytes,
    maxFrameBytes,
    maxPayloadBytes,
    framesNearBound: nearBound,
    agentTextBytes,
    totalBytesPerTextByte: agentTextBytes === 0 ? null : totalBytes / agentTextBytes,
  }
}
