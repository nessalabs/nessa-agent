/**
 * The sample experiment's results at the size real evals have: thousands of
 * test cases in slices, and changes that touch one file or a thousand. All of
 * it is derived from a run's id, so it reads the same on every launch.
 */
import type { Authored } from "./sample-experiment"
import type {
  CaseResults,
  CaseSlice,
  Change,
  FileChange,
  FileStatus,
  MovedCase,
} from "../../model/experiment"

/** A small deterministic PRNG. */
export function seeded(seed: string): () => number {
  let state = 2166136261
  for (let index = 0; index < seed.length; index += 1)
    state = Math.imul(state ^ seed.charCodeAt(index), 16777619)
  return () => {
    state = Math.imul(state ^ (state >>> 15), 2246822507)
    state = Math.imul(state ^ (state >>> 13), 3266489909)
    state ^= state >>> 16
    return (state >>> 0) / 4294967296
  }
}

/** The kinds of request the checkout suite is made of, with their share and a few of their cases. */
const slices: readonly (readonly [
  name: string,
  share: number,
  titles: readonly string[],
])[] = [
  [
    "Refunds",
    0.24,
    [
      "Refund for a damaged item",
      "Refund outside the 30-day window",
      "Refund to an expired card",
      "Refund on a gift order",
    ],
  ],
  [
    "Where is my order",
    0.18,
    [
      "Order shows delivered but never arrived",
      "Tracking stuck for a week",
      "Two parcels, one tracking number",
    ],
  ],
  [
    "Partial refunds",
    0.12,
    [
      "Refund one item of three",
      "Refund shipping only",
      "Partial refund after a price drop",
    ],
  ],
  [
    "Escalations",
    0.1,
    [
      "Refund over $200 without a manager",
      "Repeat complaint in a week",
      "Chargeback threatened",
    ],
  ],
  [
    "Cancellations",
    0.1,
    [
      "Cancel after dispatch",
      "Cancel one subscription box",
      "Cancel and reorder in another size",
    ],
  ],
  [
    "Address changes",
    0.09,
    [
      "Change address after dispatch",
      "Address missing a flat number",
      "Deliver to a pickup point",
    ],
  ],
  [
    "Promo codes",
    0.09,
    [
      "Code rejected at checkout",
      "Two codes on one order",
      "Code applied after purchase",
    ],
  ],
  [
    "Account & login",
    0.08,
    [
      "Order placed as a guest",
      "Email changed since ordering",
      "Locked out after a reset",
    ],
  ],
]

/** Which slices an area's changes move the most. */
const leaning: Record<string, readonly string[]> = {
  prompt: ["Refunds", "Escalations", "Where is my order"],
  tools: ["Partial refunds", "Where is my order", "Cancellations"],
  retrieval: ["Refunds", "Promo codes", "Address changes"],
  model: ["Escalations", "Account & login"],
  harness: ["Partial refunds", "Cancellations", "Refunds"],
}

/**
 * The test split against the parent: enough cases fixed and broken to make up
 * `delta` points over `total` cases, spread over the slices — most where the
 * run's area leans — and every one of them listed.
 */
export function caseResults(
  id: string,
  areaId: string,
  parentScore: number,
  delta: number,
  total: number,
): CaseResults {
  const random = seeded(`cases-${id}`)
  const net = Math.round((delta / 100) * total)
  const churn = Math.round(total * (0.006 + random() * 0.014))
  const fixed = Math.max(net, 0) + churn
  const broke = Math.max(-net, 0) + churn
  const passingBefore = Math.round((parentScore / 100) * total)
  const passing = Math.max(passingBefore - broke, 0)
  const failing = Math.max(total - passing - fixed - broke, 0)

  const leans = leaning[areaId] ?? []
  const weights = slices.map(
    ([name, share]) => share * (leans.includes(name) ? 2.4 : 1) * (0.7 + random() * 0.6),
  )
  const weightSum = weights.reduce((sum, each) => sum + each, 0)
  const spread = (count: number) => {
    const parts = weights.map((weight) => Math.floor((weight / weightSum) * count))
    parts[0] += count - parts.reduce((sum, each) => sum + each, 0)
    return parts
  }
  const fixedBy = spread(fixed)
  const brokeBy = spread(broke)
  const sliceTotals = slices.map(([, share]) => Math.round(share * total))
  sliceTotals[0] += total - sliceTotals.reduce((sum, each) => sum + each, 0)
  const caseSlices: CaseSlice[] = slices.map(([name], index) => {
    const size = sliceTotals[index]
    const rate = Math.min(0.97, Math.max(0.2, parentScore / 100 + (random() - 0.5) * 0.3))
    const before = Math.min(size, Math.max(brokeBy[index], Math.round(size * rate)))
    const after = Math.min(size, Math.max(0, before + fixedBy[index] - brokeBy[index]))
    return { name, total: size, before, after }
  })

  const moved: MovedCase[] = []
  const taken = new Set<number>()
  const caseId = () => {
    let number = 1 + Math.floor(random() * total)
    while (taken.has(number)) number = (number % total) + 1
    taken.add(number)
    return `C-${String(number).padStart(5, "0")}`
  }
  slices.forEach(([name, , titles], index) => {
    for (const [move, count] of [
      ["fixed", fixedBy[index]],
      ["broke", brokeBy[index]],
    ] as const)
      for (let each = 0; each < count; each += 1)
        moved.push({
          id: caseId(),
          title: titles[Math.floor(random() * titles.length)],
          slice: name,
          move,
        })
  })
  moved.sort((a, b) => a.id.localeCompare(b.id))
  return { counts: { fixed, broke, passing, failing }, slices: caseSlices, moved }
}

/** Where an area's changes land in the checkout agent's repository. */
const roots: Record<string, readonly string[]> = {
  prompt: ["prompts/system", "prompts/snippets"],
  tools: ["tools/schemas", "tools/descriptions"],
  retrieval: ["retrieval/src", "retrieval/index", "retrieval/tests"],
  model: ["config/models", "config/routing"],
  harness: [
    "harness/src/turn",
    "harness/src/tools",
    "harness/src/errors",
    "harness/tests",
    "harness/src/transport",
  ],
}

const extensions: Record<string, string> = {
  prompt: "md",
  tools: "json",
  retrieval: "ts",
  model: "yaml",
  harness: "ts",
}

const stems = [
  "refunds",
  "orders",
  "lookup",
  "search",
  "escalation",
  "policy",
  "shipment",
  "retry",
  "budget",
  "limits",
  "session",
  "validate",
  "amounts",
  "errors",
  "context",
  "stream",
  "cancel",
  "address",
  "promo",
  "account",
  "tracking",
  "window",
  "summary",
  "intent",
]

const notes: Record<string, readonly string[]> = {
  prompt: ["Tightens the wording", "Moves a rule earlier", "Adds an example"],
  tools: ["Clarifies when to use it", "Documents an error code", "Renames a parameter"],
  retrieval: [
    "Changes how chunks are ranked",
    "Adds a test case",
    "Adjusts the index settings",
  ],
  model: ["Switches the model", "Changes the effort", "Routes a kind of request"],
  harness: [
    "Passes the new option through",
    "Updates the tests",
    "Handles the new error",
  ],
}

/**
 * What a run changed: the file its agent led with, then as many more as
 * `count` asks, spread over the area's folders — one file for a prompt tweak,
 * a thousand for a sweeping harness change. Small changes carry a note per
 * file, as an agent would write; large ones only a summary.
 */
export function changeFor(
  id: string,
  areaId: string,
  written: Authored | undefined,
  count: number,
): Change {
  const random = seeded(`files-${id}`)
  const folders = roots[areaId] ?? ["src"]
  const extension = extensions[areaId] ?? "ts"
  const said = notes[areaId] ?? []
  const files: FileChange[] = []
  const seen = new Set<string>()
  if (written?.lead) {
    files.push({ ...written.lead, status: "modified" })
    seen.add(written.lead.path)
  }
  while (files.length < count) {
    const folder = folders[Math.floor(random() * folders.length)]
    const depth = count > 60 ? 1 + Math.floor(random() * 3) : 0
    const nested = Array.from(
      { length: depth },
      () => stems[Math.floor(random() * stems.length)],
    )
    const name = `${stems[Math.floor(random() * stems.length)]}${random() < 0.3 ? `-${stems[Math.floor(random() * stems.length)]}` : ""}.${extension}`
    const path = [folder, ...nested, name].join("/")
    if (seen.has(path)) continue
    seen.add(path)
    const roll = random()
    const status: FileStatus =
      roll < 0.08
        ? "added"
        : roll < 0.12
          ? "deleted"
          : roll < 0.15
            ? "renamed"
            : "modified"
    const size = Math.round(Math.pow(random(), 3) * 180) + 1
    files.push({
      path,
      status,
      added: status === "deleted" ? 0 : size,
      removed: status === "added" ? 0 : Math.round(size * random() * 0.7),
      ...(count <= 12 && said.length > 0
        ? { note: said[Math.floor(random() * said.length)] }
        : {}),
    })
  }
  return { ...(written ? { summary: written.summary } : {}), files }
}
