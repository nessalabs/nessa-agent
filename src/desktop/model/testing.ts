/**
 * What the desktop model's tests share: a seeded table of strings for the
 * property tests of what encodes ids (`id-encoding.test.ts`, and the pane
 * items' codec in the workspace). Seeded, so a failure names the string
 * that broke and runs the same again.
 */

/** A small seeded generator (mulberry32): the same seed, the same numbers. */
export function seededRandom(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/**
 * The code units the table draws from, weighted toward what breaks an
 * encoding: its own separators and escape, look-alike hex digits, both
 * halves of a surrogate pair alone, and anything at all.
 */
const draws: readonly ((random: () => number) => string)[] = [
  (random) => "aZ09-._~"[Math.floor(random() * 8)],
  (random) => ":%sw/ "[Math.floor(random() * 6)],
  (random) => "0123456789abcdefABCDEF"[Math.floor(random() * 22)],
  (random) => String.fromCharCode(0xd800 + Math.floor(random() * 0x400)),
  (random) => String.fromCharCode(0xdc00 + Math.floor(random() * 0x400)),
  () => "😀",
  (random) => String.fromCharCode(Math.floor(random() * 0x10000)),
]

/** A string of up to `maxLength` code units, drawn from `draws`. */
export function generatedString(random: () => number, maxLength = 12): string {
  const length = Math.floor(random() * (maxLength + 1))
  let text = ""
  while (text.length < length) text += draws[Math.floor(random() * draws.length)](random)
  return text
}

/** `count` generated strings from `seed`, the empty string first. */
export function generatedStrings(count: number, seed: number): string[] {
  const random = seededRandom(seed)
  return ["", ...Array.from({ length: count - 1 }, () => generatedString(random))]
}
