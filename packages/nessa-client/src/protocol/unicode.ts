/** A surrogate standing alone: `u` makes a well-formed pair one code point, which this never matches. */
const unpairedSurrogate = /\p{Surrogate}/u

/**
 * Whether `text` is Unicode: no surrogate without its partner. The gateway's
 * JSON decoder cannot read one, so a frame holding one is never answered
 * (#403). The one statement of it, for every string this client checks.
 */
export function wellFormedText(text: string): boolean {
  return !unpairedSurrogate.test(text)
}
