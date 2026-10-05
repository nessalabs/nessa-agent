/**
 * Words the window says about something it did not name itself — an MCP
 * App's server and tool (#390) — kept as words and names apart, so each name
 * is isolated from the words around it wherever it is shown: a name written
 * right to left, or one carrying direction marks, cannot reorder the
 * sentence it sits in, nor run into the name beside it (Unicode bidi
 * isolation). Drawn, a name is a `<bdi>` (`Said`); in an attribute, such as
 * an accessible name, it is set between FSI and PDI (`spoken`), the same
 * isolation in plain text.
 */

/** A sentence: its words, and the names in it that someone else chose. */
export type Said = readonly (string | { readonly name: string })[]

/** A name in a sentence, to be isolated wherever it is shown. */
export const named = (name: string) => ({ name })

/** The sentence drawn, each name in a `<bdi>`. */
export function Saying({ said }: { said: Said }) {
  return (
    <>
      {said.map((part, index) =>
        typeof part === "string" ? part : <bdi key={index}>{part.name}</bdi>,
      )}
    </>
  )
}

/** The sentence as plain text, each name between FSI (U+2068) and PDI (U+2069). */
export function spoken(said: Said): string {
  return said
    .map((part) => (typeof part === "string" ? part : `\u2068${part.name}\u2069`))
    .join("")
}
