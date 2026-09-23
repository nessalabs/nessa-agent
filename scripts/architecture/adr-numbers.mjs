/**
 * One number, one decision.
 *
 * An ADR's number is how everything else refers to it — `docs/adr/README.md`,
 * module documentation, commit messages, review threads. Two records sharing
 * one means every reference to that number is ambiguous, and neither author
 * finds out: the files have different names, so `git merge` is happy, and
 * nothing compiles a Markdown directory.
 *
 * That is not hypothetical. It happened twice, both times the same way — two
 * branches in flight, each taking the next free number from a directory that
 * cannot see what is in flight elsewhere, and whichever merged second was wrong
 * the moment it landed. CI was green on both.
 *
 * The fix is upstream of this check: a new record takes the number of the
 * GitHub issue that proposed it, and GitHub hands those out centrally, one at a
 * time, to everybody. Two branches cannot be given the same one. `0001`–`0014`
 * are the records written before that rule and keep the numbers they have; a
 * four-digit spelling means "from that era" and nothing else. Issues are past
 * 140, so the two ranges cannot meet.
 *
 * This still checks, for the mistake the rule does not prevent: a file copied
 * and half-renamed, or a number typed from memory. `todo/` and `done/` share
 * one sequence, because a decision keeps its number when it is implemented, so
 * both are read together. `0014` and `14` are the same number written two ways
 * and clash with each other. Every clash is reported rather than the first,
 * since whoever runs it is renumbering.
 *
 * Pure text on purpose: `scripts/check-architecture.mjs` runs on the Rust jobs
 * with bare Node and no `node_modules`, so nothing here may import a parser.
 */

/**
 * The leading number of an ADR filename, or `null` for anything else.
 *
 * Returned as written, so a message can quote the filename back. Use
 * [`sameNumber`] to compare two, because padding is spelling rather than
 * identity.
 */
export const adrNumber = (name) => {
  const match = /^(\d{1,5})-[a-z0-9-]+\.md$/.exec(name)
  return match ? match[1] : null
}

/** What two spellings of one number have in common. `0014` and `14` are one. */
const sameNumber = (number) => String(Number(number))

/**
 * Every number used by more than one record, with the files that use it.
 *
 * `entries` is the ADR filenames found under each directory, as
 * `[directory, names]` — passed in rather than read here so the rule stays
 * testable without a filesystem.
 */
export const duplicateAdrNumbers = (entries) => {
  const byNumber = new Map()
  for (const [directory, names] of entries) {
    for (const name of names) {
      const number = adrNumber(name)
      if (!number) continue
      const key = sameNumber(number)
      const paths = byNumber.get(key) ?? []
      paths.push(`${directory}/${name}`)
      byNumber.set(key, paths)
    }
  }
  return [...byNumber.entries()]
    .filter(([, paths]) => paths.length > 1)
    .sort(([a], [b]) => Number(a) - Number(b))
    .map(([number, paths]) => ({ number, paths: [...paths].sort() }))
}
