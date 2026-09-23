import assert from "node:assert/strict"
import { inflateSync } from "node:zlib"

export const tinyPng = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  "base64",
)

function crc32(bytes) {
  let crc = 0xffffffff
  for (const byte of bytes) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0)
  }
  return (crc ^ 0xffffffff) >>> 0
}

/** Validate the complete one-pixel PNG fixture, including CRCs and image data. */
export function assertValidTinyPng(bytes) {
  assert.deepEqual(bytes.subarray(0, 8), Buffer.from("89504e470d0a1a0a", "hex"))
  const chunks = []
  let offset = 8
  while (offset < bytes.length) {
    const length = bytes.readUInt32BE(offset)
    const type = bytes.subarray(offset + 4, offset + 8)
    const data = bytes.subarray(offset + 8, offset + 8 + length)
    const storedCrc = bytes.readUInt32BE(offset + 8 + length)
    assert.equal(crc32(Buffer.concat([type, data])), storedCrc)
    chunks.push({ type: type.toString("ascii"), data })
    offset += 12 + length
  }
  assert.equal(offset, bytes.length)
  assert.deepEqual(
    chunks.map((chunk) => chunk.type),
    ["IHDR", "IDAT", "IEND"],
  )
  assert.equal(chunks[0].data.readUInt32BE(0), 1)
  assert.equal(chunks[0].data.readUInt32BE(4), 1)
  assert.equal(chunks[0].data[8], 8)
  assert.equal(chunks[0].data[9], 4)
  assert.deepEqual(inflateSync(chunks[1].data), Buffer.from([1, 0, 255]))
}
