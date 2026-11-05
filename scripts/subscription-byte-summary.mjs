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
