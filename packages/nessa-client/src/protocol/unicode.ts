/** A surrogate standing alone: `u` makes a well-formed pair one code point, which this never matches. */
const unpairedSurrogate = /\p{Surrogate}/u

/**
 * Whether `text` is Unicode: no surrogate without its partner. A frame the
 * gateway cannot decode is answered `invalid_request` when the envelope parser
 * reads one JSON object, no decoded name appears twice, `type` is `req`, and
 * `id` is one Unicode string of 1 to 256 bytes. A name that is not Unicode is
 * not a second name, and a frame deeper than 127 containers is not read (#403).
 */
export function wellFormedText(text: string): boolean {
  return !unpairedSurrogate.test(text)
}
