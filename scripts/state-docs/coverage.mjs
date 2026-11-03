// Catalog metadata owns model identity. Prose links under Model coverage own
// shared scope; this report never infers that a model verifies an entire page.
import { posix } from "node:path"

function checkEvidence(summary) {
  const saved = summary.modelCheck
  if (!saved) return "not-run"
  if (summary.modelCheckStale) return "stale"
  if (saved.broke.length) return "broken"
  if (saved.unchecked.length) return "unchecked"
  return saved.held.length ? "held" : "empty"
}

function coverageSection(body) {
  return /^## Model coverage\s*\n([\s\S]*?)(?=^## |$(?![\s\S]))/m.exec(body)?.[1] ?? ""
}

export function modelCoverage(documents) {
  const pages = [...documents.values()]
  const byFile = new Map(pages.map((page) => [page.summary.file, page]))
  const parents = new Set(pages.map((page) => page.summary.parent))
  const entries = pages.map(({ summary, body }) => {
    const base = {
      id: summary.id,
      file: summary.file,
      title: summary.title,
      status: summary.status,
    }
    if (summary.model)
      return {
        ...base,
        disposition: "direct",
        owners: [summary.id],
        evidence: checkEvidence(summary),
      }
    if (
      summary.kind === "system" ||
      (["service", "feature"].includes(summary.kind) && parents.has(summary.id))
    )
      return { ...base, disposition: "index", owners: [] }
    if (summary.kind === "contract")
      return { ...base, disposition: "contract", owners: [] }
    if (summary.kind === "guide") return { ...base, disposition: "reference", owners: [] }
    const section = coverageSection(body)
    const owners = new Set()
    for (const match of section.matchAll(/\[[^\]]+\]\(([^\s)]+)(?:\s+"[^"]*")?\)/g)) {
      const target = match[1].split(/[?#]/)[0]
      if (!target || /^[a-z]+:/i.test(target) || target.startsWith("/")) continue
      const linked = byFile.get(
        posix.normalize(posix.join(posix.dirname(summary.file), target)),
      )
      if (linked?.summary.model) owners.add(linked.summary.id)
    }
    if (owners.size)
      return {
        ...base,
        disposition: "shared",
        owners: [...owners].sort(),
        evidence: "see-owner-scope",
      }
    if (
      section &&
      /^\s*(?:Disposition:\s*)?external(?:-source| source)?(?: boundary)?\b/im.test(
        section,
      )
    )
      return { ...base, disposition: "external", owners: [] }
    if (section && summary.status === "proposed")
      return { ...base, disposition: "proposed", owners: [] }
    return { ...base, disposition: "unmodeled", owners: [] }
  })
  const counts = Object.fromEntries(
    [...new Set(entries.map((entry) => entry.disposition))]
      .sort()
      .map((key) => [key, entries.filter((entry) => entry.disposition === key).length]),
  )
  return {
    // Index/contract/reference/boundary dispositions are not formal passes.
    ok: entries.every((entry) => entry.disposition !== "unmodeled"),
    counts,
    entries,
  }
}
