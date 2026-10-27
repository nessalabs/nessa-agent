/**
 * Tailwind's group and peer variants compile to a universal selector:
 * `.group-hover/copy:is(:where(.group/copy):hover *)`. The `*` is the
 * subject Blink invalidates, so a hovered pane restyles every descendant.
 * Naming the utility as the subject invalidates that element only. The
 * specificity stays the utility plus the pseudo-class, because the group
 * itself stays inside `:where`.
 *
 * A sibling combinator (`~*`) is left alone. Rewriting two of those on
 * one subject would nest the conditions and match a different element.
 */

const MARK = ":is(:where("

export function limitGroupSelectors(css: string): string {
  let out = ""
  let cursor = 0
  while (cursor < css.length) {
    const at = css.indexOf(MARK, cursor)
    if (at < 0) {
      out += css.slice(cursor)
      break
    }
    const subjectStart = boundaryBefore(css, at)
    const subject = css.slice(subjectStart, at)
    const parsed = parseIsWhere(css, at)
    if (!parsed || subject.includes(":is(") || !parsed.descendant) {
      out += css.slice(cursor, parsed ? parsed.end : at + MARK.length)
      cursor = parsed ? parsed.end : at + MARK.length
      continue
    }
    out += css.slice(cursor, subjectStart)
    out += `:is(:where(${parsed.ancestor})${parsed.beforeStar}${subject})`
    cursor = parsed.end
  }
  return out
}

function boundaryBefore(css: string, at: number): number {
  let start = at
  while (start > 0) {
    const char = css[start - 1]
    if (char === "," || char === "{" || char === "}" || char === ";") break
    start -= 1
  }
  return start
}

function parseIsWhere(
  css: string,
  at: number,
): { ancestor: string; beforeStar: string; descendant: boolean; end: number } | null {
  let index = at + MARK.length
  let depth = 1
  const ancestorStart = index
  while (index < css.length && depth > 0) {
    const char = css[index]
    if (char === "(") depth += 1
    else if (char === ")") depth -= 1
    if (depth > 0) index += 1
  }
  if (depth !== 0) return null
  const ancestor = css.slice(ancestorStart, index)
  index += 1
  const middleStart = index
  let middleDepth = 1
  while (index < css.length && middleDepth > 0) {
    const char = css[index]
    if (char === "(") middleDepth += 1
    else if (char === ")") middleDepth -= 1
    if (middleDepth > 0) index += 1
  }
  if (middleDepth !== 0) return null
  const middle = css.slice(middleStart, index)
  const star = middle.lastIndexOf("*")
  if (star < 0) return null
  return {
    ancestor,
    beforeStar: middle.slice(0, star),
    descendant: middle[star - 1] !== "~",
    end: index + 1,
  }
}
