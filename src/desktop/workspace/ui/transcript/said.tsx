/**
 * Words the window says about something it did not name itself — an MCP
 * App's server and tool (#390) — kept as words and names apart, so each name
 * is isolated from the words around it wherever it is shown: a name written
 * right to left cannot reorder the sentence it sits in, nor run into the name
 * beside it (Unicode bidi isolation). Drawn, a name is a `<bdi>` (`Saying`);
 * in an attribute, such as an accessible name, it is set between FSI and PDI
 * (`spoken`), the same isolation in plain text.
 *
 * Isolation holds only while the name cannot end it, nor turn its own letters
 * round: a PDI in a name closes the `<bdi>` early, and the embedding after it
 * runs on over the words that follow; an override (U+202E, RLO) shows
 * `evil`, RLO, `gnp.exe` as `evilexe.png`. So every bidi control a name
 * carries — the embeddings and overrides (U+202A–U+202E), the isolates
 * (U+2066–U+2069), the marks (U+200E, U+200F, U+061C), and the line and
 * paragraph separators (U+2028, U+2029) — is shown as
 * U+FFFD, the replacement character, before the name is isolated
 * (`shownName`): seen, and doing nothing. A sentence that repeats those names
 * (`naming`) isolates each of them the same way, and leaves every other word.
 */
import { bidiControls } from "./bidi-controls.mjs"

/** A sentence: its words, and the names in it that someone else chose. */
export type Said = readonly (string | { readonly name: string })[]

/** A name in a sentence, to be isolated wherever it is shown. */
export const named = (name: string) => ({ name })

const isBidi = (char: string): boolean => bidiControls.test(char)

/** A name as it is shown: each bidi control it carries replaced by U+FFFD. */
export const shownName = (name: string): string =>
  [...name].map((char) => (isBidi(char) ? "\uFFFD" : char)).join("")

const unicodeEscape = (char: string): string => {
  const code = char.codePointAt(0) ?? 0
  return `\\u${code.toString(16).padStart(4, "0")}`
}

/**
 * JSON as it is shown. A bidi control, or a line or paragraph separator,
 * inside a string becomes its `\u` escape. One outside a string — not a
 * value, and not valid JSON — becomes
 * U+FFFD, so an escape there is not left for a reader to take as syntax.
 * `JSON.parse` of the result equals `JSON.parse` of `json` when `json` parses
 * (`said.test.ts`).
 */
export function shownJson(json: string): string {
  let out = ""
  let inString = false
  let escaped = false
  for (const char of json) {
    if (inString) {
      if (escaped) {
        escaped = false
        out += char
        continue
      }
      if (char === "\\") {
        escaped = true
        out += char
        continue
      }
      if (char === '"') {
        inString = false
        out += char
        continue
      }
      out += isBidi(char) ? unicodeEscape(char) : char
      continue
    }
    if (char === '"') {
      inString = true
      out += char
      continue
    }
    out += isBidi(char) ? "\uFFFD" : char
  }
  return out
}

const isJsonArgument = (text: string): boolean => {
  try {
    const value = JSON.parse(text) as unknown
    return value !== null && typeof value === "object"
  } catch {
    return false
  }
}

export function shownCommand(command: string): string {
  const space = command.indexOf(" ")
  if (space > 0) {
    const name = command.slice(0, space)
    const rest = command.slice(space + 1)
    if (isJsonArgument(rest)) return `${shownName(name)} ${shownJson(rest)}`
  }
  return [...command].map((char) => (isBidi(char) ? unicodeEscape(char) : char)).join("")
}

/**
 * A sentence that repeats names someone else chose, each isolated wherever
 * it is shown (`Saying`, `spoken`). The earliest name is taken, and where
 * two start together the longer one, so a name that contains another stays
 * whole. A name of no letters is not one. Words that are not those names
 * are left as they are (`approval-request.test.tsx`).
 */
export function naming(text: string, names: readonly string[]): Said {
  const sought = names.filter((name) => name.length > 0)
  if (sought.length === 0 || text.length === 0) return [text]
  const parts: (string | { readonly name: string })[] = []
  let cursor = 0
  while (cursor < text.length) {
    let match: { readonly name: string; readonly at: number } | undefined
    for (const name of sought) {
      const at = text.indexOf(name, cursor)
      if (at === -1) continue
      if (
        match === undefined ||
        at < match.at ||
        (at === match.at && name.length > match.name.length)
      )
        match = { name, at }
    }
    if (match === undefined) {
      parts.push(text.slice(cursor))
      break
    }
    if (match.at > cursor) parts.push(text.slice(cursor, match.at))
    parts.push(named(match.name))
    cursor = match.at + match.name.length
  }
  return parts
}

/** The sentence drawn, each name in a `<bdi>`. */
export function Saying({ said }: { said: Said }) {
  return (
    <>
      {said.map((part, index) =>
        typeof part === "string" ? part : <bdi key={index}>{shownName(part.name)}</bdi>,
      )}
    </>
  )
}

/** The sentence as plain text, each name between FSI (U+2068) and PDI (U+2069). */
export function spoken(said: Said): string {
  return said
    .map((part) =>
      typeof part === "string" ? part : `\u2068${shownName(part.name)}\u2069`,
    )
    .join("")
}
