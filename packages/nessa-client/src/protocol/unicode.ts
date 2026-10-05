/** A surrogate standing alone: `u` makes a well-formed pair one code point, which this never matches. */
const unpairedSurrogate = /\p{Surrogate}/u

/**
 * Whether `text` is Unicode: no surrogate without its partner. The gateway
 * answers `invalid_request` for a frame that carries one when the request id
 * itself can still be read (#403).
 */
export function wellFormedText(text: string): boolean {
  return !unpairedSurrogate.test(text)
}
