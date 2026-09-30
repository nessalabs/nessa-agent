/**
 * The sample experiment: six agents hill-climbing a checkout-support agent
 * for three hours. Runs are written as changes against the best version at
 * the time they were proposed, and their scores derived from it, so the
 * lineage and the numbers always agree. Dated relative to `now`.
 */
import type {
  Agent,
  Area,
  Experiment,
  Note,
  Run,
  SettledVerdict,
} from "../../model/experiment"
import { caseResults, changeFor } from "./sample-scale"

const minute = 60_000

export const sampleExperimentId = "checkout-hillclimb"

export const areas: readonly Area[] = [
  {
    id: "prompt",
    name: "System prompt",
    hypothesis: "Spell out refund windows, escalation and order of checks.",
    slot: 1,
  },
  {
    id: "tools",
    name: "Tool descriptions",
    hypothesis: "Say when each tool is the right one, and what its errors mean.",
    slot: 2,
  },
  {
    id: "retrieval",
    name: "Policy retrieval",
    hypothesis: "Smaller, titled policy chunks, found by intent.",
    slot: 3,
  },
  {
    id: "model",
    name: "Model & effort",
    hypothesis: "Find where more thinking pays for itself.",
    slot: 4,
  },
  {
    id: "harness",
    name: "Harness",
    hypothesis: "Fewer wasted turns: retries, parallel lookups, validation.",
    slot: 5,
  },
]

type Planned = readonly [
  area: string,
  agent: string,
  title: string,
  rationale: string,
  train: number,
  test: number,
  verdict: SettledVerdict,
  minutesAgo: number,
  costChange?: number,
]

/** The settled runs, in proposal order: deltas against the best at the time. */
const history: readonly Planned[] = [
  [
    "prompt",
    "wren",
    "State the 30-day refund window up front",
    "Half the failed refunds cite the wrong window.",
    1.4,
    0.9,
    "flat",
    182,
  ],
  [
    "tools",
    "juno",
    "Rename search_orders to find_orders_by_customer",
    "The agent reaches for search when it has an order number.",
    0.6,
    -0.3,
    "flat",
    176,
  ],
  [
    "prompt",
    "tamsin",
    "Add an escalation checklist for refunds over $200",
    "Large refunds are approved without a manager check.",
    3.6,
    3.1,
    "kept",
    171,
  ],
  [
    "retrieval",
    "orla",
    "Split policy docs into titled 300-token chunks",
    "Answers quote the wrong section of long policies.",
    1.1,
    0.4,
    "flat",
    165,
  ],
  [
    "model",
    "pike",
    "Opus 5.5 at high effort",
    "Multi-step refunds fail on planning, not knowledge.",
    2.4,
    2.0,
    "costly",
    160,
    0.64,
  ],
  [
    "harness",
    "sable",
    "Retry lookup_order once on timeout",
    "7% of failures are a single transient timeout.",
    0.8,
    0.5,
    "flat",
    154,
  ],
  [
    "tools",
    "juno",
    "Say when lookup_order beats search_orders",
    "Wrong tool on 14 of 41 failing train cases.",
    2.9,
    2.3,
    "kept",
    149,
  ],
  [
    "prompt",
    "wren",
    "Few-shot: two refund conversations",
    "Show the shape of a good refund instead of describing it.",
    3.8,
    -0.6,
    "overfit",
    143,
  ],
  [
    "retrieval",
    "orla",
    "Rerank policy chunks by section title",
    "Titles carry the policy's intent.",
    -1.2,
    -1.9,
    "regressed",
    138,
  ],
  [
    "harness",
    "sable",
    "Run order lookups in parallel",
    "Customers with several orders time out the turn.",
    2.2,
    1.9,
    "kept",
    132,
  ],
  [
    "model",
    "pike",
    "Sonnet 5.5 at medium effort",
    "Check how much of the score is the model.",
    -3.4,
    -4.1,
    "regressed",
    127,
    -0.38,
  ],
  [
    "prompt",
    "tamsin",
    "Ask for the order number before anything else",
    "Most wasted turns are searching without one.",
    0.4,
    0.2,
    "flat",
    121,
  ],
  [
    "tools",
    "juno",
    "Document issue_refund's partial-amount field",
    "Partial refunds are issued in full.",
    1.9,
    1.6,
    "kept",
    116,
  ],
  [
    "retrieval",
    "orla",
    "Include each policy's effective date in its chunk",
    "Superseded policies are quoted as current.",
    0.3,
    -0.2,
    "flat",
    110,
  ],
  [
    "prompt",
    "wren",
    "Quote policy text verbatim",
    "Paraphrased policy is graded as wrong.",
    2.6,
    0.1,
    "overfit",
    105,
  ],
  [
    "harness",
    "sable",
    "Surface tool errors to the model as text",
    "Silent tool failures read as empty results.",
    0.9,
    0.6,
    "flat",
    99,
  ],
  [
    "model",
    "pike",
    "Opus 5.5 at medium effort",
    "The middle of the curve, for cost.",
    1.1,
    0.7,
    "flat",
    94,
    0.22,
  ],
  [
    "prompt",
    "tamsin",
    "Confirm the resolution back to the customer",
    "The grader's rubric asks for a confirmed outcome.",
    1.8,
    1.4,
    "kept",
    88,
  ],
  [
    "tools",
    "juno",
    "Merge get_shipment into lookup_order",
    "One lookup instead of two.",
    -0.8,
    -1.4,
    "regressed",
    83,
  ],
  [
    "retrieval",
    "orla",
    "Retrieve by intent, not by keywords",
    "Queries are the customer's words, policies are not.",
    1.5,
    1.2,
    "kept",
    77,
  ],
  [
    "harness",
    "sable",
    "Cap tool calls per turn at 12",
    "A few runaway turns dominate cost.",
    -0.2,
    0.1,
    "flat",
    72,
    -0.05,
  ],
  [
    "model",
    "pike",
    "Fable 5.1 at high effort, escalations only",
    "Spend thinking only where it is needed.",
    2.1,
    1.7,
    "costly",
    66,
    0.41,
  ],
  [
    "prompt",
    "wren",
    "Drop the persona paragraph",
    "Shorter prompt, same behaviour.",
    0.2,
    0.3,
    "flat",
    61,
    -0.04,
  ],
  [
    "tools",
    "juno",
    "Return order status as an enum, not prose",
    "Status prose is misread as a promise.",
    1.0,
    0.8,
    "flat",
    55,
  ],
  [
    "retrieval",
    "orla",
    "Add FAQ answers as a second index",
    "FAQ answers are pre-written for common cases.",
    2.2,
    -0.4,
    "overfit",
    50,
  ],
  [
    "prompt",
    "tamsin",
    "Handle 'where is my order' before refunds",
    "WISMO questions get refund answers.",
    2.3,
    1.8,
    "kept",
    44,
  ],
  [
    "harness",
    "sable",
    "Stream tool results into the context",
    "Long results crowd out the policy.",
    -0.9,
    -2.0,
    "regressed",
    39,
  ],
  [
    "model",
    "pike",
    "Route status checks to Haiku 4.5",
    "Status checks need no reasoning.",
    -0.4,
    -0.6,
    "flat",
    33,
    -0.31,
  ],
  [
    "tools",
    "juno",
    "Warn when a refund exceeds the order total",
    "Over-refunds are the costliest failure.",
    0.7,
    0.2,
    "flat",
    28,
  ],
  [
    "retrieval",
    "orla",
    "Overlap chunks by 64 tokens",
    "Rules split across chunk edges.",
    0.1,
    -0.3,
    "flat",
    22,
  ],
  [
    "prompt",
    "wren",
    "Reorder rules: safety, policy, then tone",
    "Tone rules were overriding policy.",
    1.9,
    0.4,
    "overfit",
    17,
  ],
  [
    "harness",
    "sable",
    "Retry issue_refund on 409 with backoff",
    "Conflicts are retried by hand today.",
    0.6,
    0.4,
    "flat",
    12,
  ],
]

type Live = readonly [
  area: string,
  agent: string,
  title: string,
  rationale: string,
  minutesAgo: number,
  done: number,
]

/** Being evaluated now, against the best so far. */
const live: readonly Live[] = [
  [
    "prompt",
    "tamsin",
    "Ask one clarifying question when intent is unclear",
    "Ambiguous asks are answered with a guess.",
    9,
    84,
  ],
  [
    "tools",
    "juno",
    "Describe error codes in lookup_order's schema",
    "404 and 410 are treated the same.",
    7,
    132,
  ],
  [
    "retrieval",
    "orla",
    "Hybrid search: keywords and embeddings",
    "Intent search misses exact policy names.",
    4,
    41,
  ],
  [
    "harness",
    "sable",
    "Validate refund amounts before calling the tool",
    "Invalid amounts cost a whole turn to recover from.",
    6,
    150,
  ],
]

const queued: readonly (readonly [
  area: string,
  agent: string,
  title: string,
  rationale: string,
])[] = [
  [
    "prompt",
    "wren",
    "One short refund example instead of rules",
    "The last three prompt rules overfit; try showing instead.",
  ],
  [
    "model",
    "pike",
    "Opus 5.5 at low effort with richer tool docs",
    "Buy back the effort's gain with better descriptions.",
  ],
]

const agentNames: Record<string, string> = {
  wren: "Wren",
  juno: "Juno",
  orla: "Orla",
  pike: "Pike",
  sable: "Sable",
  tamsin: "Tamsin",
}

/** What an agent wrote about its change: a sentence, and the file it led with. */
export interface Authored {
  readonly summary: string
  readonly lead?: {
    readonly path: string
    readonly note: string
    readonly added: number
    readonly removed: number
  }
}

const authored: Record<number, Authored> = {
  3: {
    summary:
      "Adds a three-step check before any refund over $200, ending in an escalation.",
    lead: {
      path: "prompts/system/refunds.md",
      note: "New checklist under Refunds: window, dispute, escalate",
      added: 4,
      removed: 0,
    },
  },
  7: {
    summary: "Rewrites lookup_order's description to say when it beats search_orders.",
    lead: {
      path: "tools/descriptions/lookup_order.md",
      note: "Use it whenever there is an order number",
      added: 3,
      removed: 1,
    },
  },
  10: {
    summary: "Looks up every order in a turn at once instead of one after another.",
    lead: {
      path: "harness/src/turn/lookups.ts",
      note: "Sequential lookups become one Promise.all",
      added: 1,
      removed: 1,
    },
  },
  13: {
    summary:
      "Describes issue_refund's amount as the part to return, and full refunds as leaving it out.",
    lead: {
      path: "tools/schemas/issue_refund.json",
      note: "Adds a description to amount",
      added: 5,
      removed: 1,
    },
  },
  16: {
    summary:
      "Turns every tool failure into a sentence the model can read, across all tool adapters and their tests.",
  },
  18: {
    summary: "Ends each reply by saying what was done and what happens next.",
    lead: {
      path: "prompts/system/closing.md",
      note: "New closing rule with an example",
      added: 2,
      removed: 0,
    },
  },
  20: {
    summary: "Retrieves policy by the customer's intent rather than their words.",
    lead: {
      path: "retrieval/src/query.ts",
      note: "Queries with the classified intent",
      added: 2,
      removed: 1,
    },
  },
  26: {
    summary: "Answers where-is-my-order questions before anything about refunds.",
    lead: {
      path: "prompts/system/order-of-checks.md",
      note: "WISMO first, refunds only when asked",
      added: 2,
      removed: 0,
    },
  },
  27: {
    summary:
      "Streams tool results into the context as they arrive, through the turn loop, every adapter, and the transport.",
  },
}

const round = (value: number) => Math.round(value * 10) / 10

/**
 * How many files a run touched: one or two for a prompt or a model, more for
 * the harness — and two sweeping changes, so the views are seen at scale.
 */
export function fileCount(number: number, areaId: string): number {
  if (number === 16) return 214
  if (number === 27) return 1240
  switch (areaId) {
    case "prompt":
      return 1 + (number % 2)
    case "tools":
      return 1 + (number % 3)
    case "retrieval":
      return 2 + (number % 5)
    case "model":
      return 1 + (number % 2)
    default:
      return 3 + (number % 12)
  }
}

const baselineCost = 0.052
const suite = {
  name: "checkout-support v3",
  train: 6000,
  test: 2400,
  grader: "Rubric judge + refund-amount check",
}

export function sampleExperiment(now: number): Experiment {
  const ago = (minutes: number) => now - minutes * minute
  const baseline: Run = {
    id: "r0",
    number: 0,
    parentId: null,
    areaId: "prompt",
    agentId: "tamsin",
    title: "Baseline: production prompt and tools",
    rationale: "What ships today, scored twice to measure the noise.",
    change: { files: [] },
    startedAt: ago(190),
    verdict: "kept",
    train: { mean: 57.9, ci: 2.4 },
    test: { mean: 58.3, ci: 1.9 },
    cost: baselineCost,
  }
  let best = baseline
  const runs: Run[] = []
  history.forEach(
    (
      [areaId, agentId, title, rationale, train, test, verdict, minutesAgo, costChange],
      index,
    ) => {
      const number = index + 1
      const parent = best
      const trainMean = round((parent.train?.mean ?? 0) + train)
      const testMean = round((parent.test?.mean ?? 0) + test)
      const run: Run = {
        id: `r${number}`,
        number,
        parentId: parent.id,
        areaId,
        agentId,
        title,
        rationale,
        change: changeFor(
          `r${number}`,
          areaId,
          authored[number],
          fileCount(number, areaId),
        ),
        startedAt: ago(minutesAgo),
        verdict,
        train: { mean: trainMean, ci: 2.1 + (number % 4) * 0.1 },
        test: { mean: testMean, ci: 1.7 + (number % 3) * 0.1 },
        cost:
          Math.round(
            (parent.cost ?? baselineCost) *
              (1 + (costChange ?? (number % 5) * 0.01 - 0.02)) *
              1000,
          ) / 1000,
        cases: caseResults(
          `r${number}`,
          areaId,
          parent.test?.mean ?? 0,
          test,
          suite.test,
        ),
      }
      runs.push(run)
      if (verdict === "kept") best = run
    },
  )
  let number = runs.length
  for (const [areaId, agentId, title, rationale, minutesAgo, done] of live) {
    number += 1
    runs.push({
      id: `r${number}`,
      number,
      parentId: best.id,
      areaId,
      agentId,
      title,
      rationale,
      change: changeFor(`r${number}`, areaId, undefined, fileCount(number, areaId)),
      startedAt: ago(minutesAgo),
      verdict: "running",
      // The sample's progress is written against 180 cases; scaled to the suite.
      progress: {
        done: Math.round((done / 180) * (suite.train + suite.test)),
        total: suite.train + suite.test,
      },
    })
  }
  for (const [areaId, agentId, title, rationale] of queued) {
    number += 1
    runs.push({
      id: `r${number}`,
      number,
      parentId: best.id,
      areaId,
      agentId,
      title,
      rationale,
      change: changeFor(`r${number}`, areaId, undefined, fileCount(number, areaId)),
      startedAt: ago(1),
      verdict: "queued",
    })
  }
  const runOf = (agentId: string) =>
    runs.find((run) => run.agentId === agentId && run.verdict === "running")
  const agents: Agent[] = Object.entries(agentNames).map(([id, name]) => {
    const running = runOf(id)
    const areaId =
      running?.areaId ??
      runs.filter((run) => run.agentId === id).at(-1)?.areaId ??
      "prompt"
    if (running)
      return {
        id,
        name,
        areaId,
        activity: { kind: "evaluating", runId: running.id },
        since: running.startedAt,
      }
    if (id === "pike")
      return {
        id,
        name,
        areaId: "model",
        activity: {
          kind: "diagnosing",
          note: "Three rounds without a keep — every gain costs 40% more. Reading the remaining train failures by root cause.",
        },
        since: ago(3),
      }
    return {
      id,
      name,
      areaId,
      activity: {
        kind: "drafting",
        note: "A shorter refund section with one worked example.",
      },
      since: ago(2),
    }
  })
  const notes: Note[] = [
    {
      id: "n-ceiling",
      tone: "info",
      text: "Headroom confirmed: Opus 5.5 at max effort scores 84.1 on test, well clear of the baseline's 58.3.",
      at: ago(186),
    },
    {
      id: "n-noise",
      tone: "info",
      text: "Noise floor is ±1.2 points, from two reruns of the baseline. Moves inside it count as flat.",
      at: ago(185),
    },
    {
      id: "n-retrieval",
      tone: "good",
      text: "Policy retrieval kept its first change on its fifth attempt: retrieving by intent.",
      at: ago(77),
      runId: "r20",
    },
    {
      id: "n-model",
      tone: "warning",
      text: "Model & effort has gone three rounds without a keep: its gains cost 22–64% more per task. Pike is diagnosing.",
      at: ago(33),
      runId: "r28",
    },
    {
      id: "n-overfit",
      tone: "warning",
      text: "Third overfit from the prompt area (#8, #15, #31): rule-style edits fit the train cases. Wren is switching to an example.",
      at: ago(17),
      runId: "r31",
    },
  ]
  return {
    id: sampleExperimentId,
    sessionId: "checkout-hillclimb",
    title: "Checkout support agent",
    goal: "Raise task success on checkout support without raising cost per task by more than 10%.",
    metric: "Task success",
    suite,
    noise: 1.2,
    ceiling: 84.1,
    budget: 60,
    startedAt: ago(190),
    asOf: now,
    baseline,
    runs,
    areas,
    agents,
    notes,
  }
}

/** What a live run will score when its last case is in, and the next change its agent tries. */
export interface Scripted {
  readonly train: number
  readonly test: number
  readonly verdict: SettledVerdict
}

export const liveOutcomes: Record<string, Scripted> = {
  "ask one clarifying question when intent is unclear": {
    train: 2.4,
    test: 0.3,
    verdict: "overfit",
  },
  "describe error codes in lookup_order's schema": {
    train: 0.7,
    test: 0.5,
    verdict: "flat",
  },
  "hybrid search: keywords and embeddings": { train: 1.9, test: 1.5, verdict: "kept" },
  "validate refund amounts before calling the tool": {
    train: 2.0,
    test: 1.6,
    verdict: "kept",
  },
  "one short refund example instead of rules": { train: 2.1, test: 1.9, verdict: "kept" },
  "opus 5.5 at low effort with richer tool docs": {
    train: 0.9,
    test: 0.8,
    verdict: "flat",
  },
}

/** What each area's agent tries after its current change settles. */
export const nextIdeas: Record<
  string,
  readonly (readonly [title: string, rationale: string, outcome: Scripted])[]
> = {
  prompt: [
    [
      "Name the three refund outcomes explicitly",
      "The grader looks for one of three named outcomes.",
      { train: 1.2, test: 0.6, verdict: "flat" },
    ],
    [
      "Put the order of checks in a numbered list",
      "Checks are skipped when they sit mid-paragraph.",
      { train: 1.7, test: 1.3, verdict: "kept" },
    ],
    [
      "Tell it what to do when the policy is silent",
      "Edge cases get invented policy.",
      { train: 2.2, test: 0.2, verdict: "overfit" },
    ],
  ],
  tools: [
    [
      "Mark search_orders results as partial",
      "The agent treats a page of results as all of them.",
      { train: 1.6, test: 1.4, verdict: "kept" },
    ],
    [
      "Give issue_refund an idempotency key",
      "Retries issue the same refund twice.",
      { train: 0.5, test: 0.3, verdict: "flat" },
    ],
    [
      "Describe get_customer's loyalty tier field",
      "Loyalty exceptions are missed.",
      { train: 0.9, test: -0.2, verdict: "flat" },
    ],
  ],
  retrieval: [
    [
      "Drop superseded policies from the index",
      "Two policies disagree on the window.",
      { train: 0.4, test: 0.2, verdict: "flat" },
    ],
    [
      "Retrieve the three best chunks, not five",
      "Extra chunks bring contradicting rules.",
      { train: 1.4, test: 1.3, verdict: "kept" },
    ],
    [
      "Summarise each policy into its first chunk",
      "The rule is often at the end of a policy.",
      { train: 1.8, test: -0.5, verdict: "overfit" },
    ],
  ],
  harness: [
    [
      "Stop after a refund is confirmed",
      "Turns keep going after the task is done.",
      { train: 0.3, test: -1.6, verdict: "regressed" },
    ],
    [
      "Cache order lookups within a turn",
      "The same order is fetched four times.",
      { train: 0.2, test: 0.1, verdict: "flat" },
    ],
    [
      "Ask before refunds over the order total",
      "The costliest failures are silent over-refunds.",
      { train: 1.5, test: 1.4, verdict: "kept" },
    ],
  ],
  model: [
    [
      "Sonnet 5.5 at high effort with the current prompt",
      "The prompt may now carry what effort used to.",
      { train: 0.6, test: -0.4, verdict: "flat" },
    ],
    [
      "Opus 5.5 at medium effort, escalations only",
      "Spend thinking where the tail is.",
      { train: 1.6, test: 1.3, verdict: "costly" },
    ],
  ],
}
