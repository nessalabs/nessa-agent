/**
 * One number, one decision.
 *
 * An ADR's number is how everything else refers to it — `docs/adr/README.md`,
 * module documentation, commit messages, review threads. Two records sharing
 * one means every reference to that number is ambiguous, and neither author
 * finds out: the files have different names, so `git merge` is happy, and
 * nothing compiles a Markdown directory.
 *
 * That is not hypothetical. It has happened twice, both times the same way —
 * two branches in flight, each taking the next free number, and whichever
 * merges second is wrong the moment it lands. CI was green on both.
 *
 * `todo/` and `done/` share the sequence, because a decision keeps its number
 * when it is implemented. So the check reads both and refuses a repeat across
 * either, and it reports every clash at once rather than the first, since
 * whoever runs it is renumbering.
 *
 * Pure text on purpose: `scripts/check-architecture.mjs` runs on the Rust jobs
 * with bare Node and no `node_modules`, so nothing here may import a parser.
 */

/** The leading number of an ADR filename, or `null` for anything else. */
export const adrNumber = (name) => {
  const match = /^(\d{4})-[a-z0-9-]+\.md$/.exec(name)
  return match ? match[1] : null
}

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
      const paths = byNumber.get(number) ?? []
      paths.push(`${directory}/${name}`)
      byNumber.set(number, paths)
    }
  }
  return [...byNumber.entries()]
    .filter(([, paths]) => paths.length > 1)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([number, paths]) => ({ number, paths: [...paths].sort() }))
}
