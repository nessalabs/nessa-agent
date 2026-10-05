/** A surrogate standing alone: `u` makes a well-formed pair one code point, which this never matches. */
const unpairedSurrogate = /\p{Surrogate}/u

/**
 * Whether `text` is Unicode: no surrogate without its partner. On the
 * authenticated socket the gateway answers `invalid_request` when `type` is
 * `req` and `id` is one Unicode string of 1 to 256 bytes (#403).
 */
export function wellFormedText(text: string): boolean {
  return !unpairedSurrogate.test(text)
}
