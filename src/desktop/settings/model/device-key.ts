/**
 * The device key's fingerprint, as the gateway compares it: SHA-256 of the
 * RFC 8410 Ed25519 SubjectPublicKeyInfo (the fixed prefix, then the 32-byte
 * key), hex. The raw key is not what is shown. One function owns the prefix.
 */

/** An Ed25519 device key is 32 bytes on the wire. */
export const DEVICE_KEY_BYTES = 32

/** An invitation id is 16 bytes on the wire. */
export const INVITATION_BYTES = 16

/**
 * The prefix of an Ed25519 SubjectPublicKeyInfo, before the 32-byte key.
 * The same bytes the gateway pins (`SPKI_PREFIX`).
 */
export const ED25519_SPKI_PREFIX = Uint8Array.from([
  0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
])

/** Lower-case hex of each byte. Empty for none. */
export function bytesToHex(bytes: readonly number[]): string {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("")
}

/** Bytes of a hex string, or `undefined` when it is not hex. */
export function hexToBytes(hex: string): number[] | undefined {
  if (hex.length % 2 !== 0 || !/^[0-9a-f]+$/i.test(hex)) return undefined
  const bytes: number[] = []
  for (let at = 0; at < hex.length; at += 2)
    bytes.push(Number.parseInt(hex.slice(at, at + 2), 16))
  return bytes
}

/**
 * SHA-256 of the SubjectPublicKeyInfo, lower-case hex. Refuses a key that is
 * not {@link DEVICE_KEY_BYTES} long: a fingerprint of something else would
 * not be the one the other device shows.
 */
export async function deviceKeyFingerprint(key: Uint8Array): Promise<string> {
  if (key.length !== DEVICE_KEY_BYTES) throw new Error("device key is not 32 bytes")
  const spki = new Uint8Array(ED25519_SPKI_PREFIX.length + key.length)
  spki.set(ED25519_SPKI_PREFIX, 0)
  spki.set(key, ED25519_SPKI_PREFIX.length)
  const digest = await crypto.subtle.digest("SHA-256", spki)
  return bytesToHex([...new Uint8Array(digest)])
}
