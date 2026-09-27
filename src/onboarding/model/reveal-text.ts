/**
 * Splitting a line into the pieces it is revealed in.
 *
 * Whole words arrive in too few, too large steps to read as a line being said;
 * single letters arrive in too many, and the eye stops reading and starts
 * counting. A couple of characters at a time is the size where the movement is
 * continuous and the words are still words.
 */

/** How many characters are revealed together, unless a caller says otherwise. */
export const REVEAL_CHUNK = 2

/**
 * Split `text` into successive chunks of `size` characters.
 *
 * Whitespace rides along inside the chunks rather than being split out, so the
 * pieces reassemble into exactly the original line — a reveal that drops or
 * moves a space is a reveal that changes the sentence.
 *
 * Counted in code points rather than UTF-16 units, so a character outside the
 * basic plane is one character here too and cannot be torn in half.
 */
export function revealChunks(text: string, size: number = REVEAL_CHUNK): string[] {
  const characters = Array.from(text)
  if (size < 1) return characters.length > 0 ? [text] : []
  const chunks: string[] = []
  for (let at = 0; at < characters.length; at += size) {
    chunks.push(characters.slice(at, at + size).join(""))
  }
  return chunks
}
