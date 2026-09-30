/**
 * The desktop's one id encoder: writes any string in a small alphabet, so an
 * id from outside — a session's, a plugin's, a widget's — can be joined to
 * others by a separator it can never contain.
 *
 * Each UTF-16 code unit is written as itself when it is one of the URI's
 * unreserved characters (`A`–`Z`, `a`–`z`, `0`–`9`, `-`, `.`, `_`, `~`), and
 * as `%` and its four uppercase hex digits otherwise. Code units, not code
 * points: a lone surrogate is a string too, and it encodes where
 * `encodeURIComponent` would throw.
 *
 * The encoding is one-to-one and canonical: every string has exactly one
 * encoding, and `decodeId` reads back only what `encodeId` writes — a
 * lowercase digit, an escaped unreserved character, or a stray `%` is not an
 * encoding of anything (`id-encoding.test.ts`).
 */

/** Whether a code unit is written as itself. */
function unreserved(unit: number): boolean {
  return (
    (unit >= 0x30 && unit <= 0x39) || // 0-9
    (unit >= 0x41 && unit <= 0x5a) || // A-Z
    (unit >= 0x61 && unit <= 0x7a) || // a-z
    unit === 0x2d || // -
    unit === 0x2e || // .
    unit === 0x5f || // _
    unit === 0x7e // ~
  )
}

/** Four uppercase hex digits: the only digits an escape is written with. */
const escapeDigits = /^[0-9A-F]{4}$/

/** Writes `id` in the unreserved characters and `%XXXX` escapes. */
export function encodeId(id: string): string {
  let encoded = ""
  for (let at = 0; at < id.length; at++) {
    const unit = id.charCodeAt(at)
    encoded += unreserved(unit)
      ? id[at]
      : `%${unit.toString(16).toUpperCase().padStart(4, "0")}`
  }
  return encoded
}

/**
 * The string `encodeId` wrote as `encoded`, or `null` when it wrote no such
 * thing: anything outside the alphabet, an escape not of four uppercase hex
 * digits, or an escape of a character that is written as itself.
 */
export function decodeId(encoded: string): string | null {
  let id = ""
  for (let at = 0; at < encoded.length;) {
    const unit = encoded.charCodeAt(at)
    if (unreserved(unit)) {
      id += encoded[at]
      at += 1
      continue
    }
    if (encoded[at] !== "%") return null
    const digits = encoded.slice(at + 1, at + 5)
    if (!escapeDigits.test(digits)) return null
    const escaped = Number.parseInt(digits, 16)
    if (unreserved(escaped)) return null
    id += String.fromCharCode(escaped)
    at += 5
  }
  return id
}
