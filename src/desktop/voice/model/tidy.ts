/**
 * The person's transcribed words, tidied before they go to the agent:
 * filler words ("um", "uh", "erm", "hmm") and a word said twice in a row
 * are dropped, spaces before punctuation and runs of spaces are closed,
 * and the first letter is made upper case. Nothing else changes: no word
 * is replaced and none is added, so what the agent reads is what was said.
 * The transcription model is asked for clean text too; this is the part
 * that cannot reword anything.
 */
const FILLER = /^(?:u+m+|u+h+|e+r+m+|h+m+|m+h*m+)$/i

/** One word with what clings to it: leading and trailing punctuation. */
const TOKEN = /^([^\p{L}\p{N}]*)([\p{L}\p{N}][\p{L}\p{N}'’-]*)?([^\p{L}\p{N}]*)$/u

export function tidy(text: string): string {
  const kept: string[] = []
  let previousWord = ""
  for (const raw of text.split(/\s+/)) {
    if (raw === "") continue
    const match = TOKEN.exec(raw)
    const word = match?.[2] ?? ""
    const trailing = match?.[3] ?? ""
    if (word && FILLER.test(word)) {
      // A filler's own punctuation goes with it, except what ends a
      // sentence, which stays on the word before.
      if (/[.?!]/.test(trailing) && kept.length > 0)
        kept[kept.length - 1] =
          kept[kept.length - 1].replace(/[,;:]*$/, "") + trailing.replace(/[^.?!]/g, "")
      continue
    }
    // "the the": the repeat goes; "had had" is rare enough to lose.
    if (word && word.toLowerCase() === previousWord && !(match?.[1] ?? "")) {
      if (trailing)
        kept[kept.length - 1] =
          kept[kept.length - 1].replace(/[^\p{L}\p{N}'’-]*$/u, "") + trailing
      continue
    }
    kept.push(raw)
    if (word) previousWord = word.toLowerCase()
    if (/[.?!]$/.test(raw)) previousWord = ""
  }
  const joined = kept
    .join(" ")
    .replace(/\s+([,.;:?!])/g, "$1")
    .replace(/^[,;:\s]+/, "")
    .trim()
  return joined.charAt(0).toUpperCase() + joined.slice(1)
}
