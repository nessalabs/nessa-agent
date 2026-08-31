const TITLE_LENGTH = 24

export function titleFor(prompt: string) {
  const trimmed = prompt.trim()
  return trimmed.length > TITLE_LENGTH
    ? `${trimmed.slice(0, TITLE_LENGTH).trimEnd()}…`
    : trimmed
}
