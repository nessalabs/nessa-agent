/**
 * A thread report: the turn the gateway sends a conversation when a thread it
 * messaged finishes (`crates/nessa-server/.../thread_reports.rs`). The agent
 * reads the whole report; the window shows it as one line naming the thread
 * and how its turn ended, never the reply or the ids.
 *
 * The report's first line is the gateway's own heading, a contract both sides
 * test against the same cases (`crates/nessa-server/tests/fixtures/
 * thread-report.json`). Only that line is read: nothing of the reply below it.
 */

/** How the thread's turn ended, as the report says it. */
export type ThreadReportOutcome =
  | "completed"
  | "failed"
  | "interrupted"
  | "cancelled"
  | "refused"
  | "output_limit"
  | "request_limit"

export interface ThreadReport {
  /** The thread the report is about: a conversation id. */
  readonly thread: string
  readonly outcome: ThreadReportOutcome
}

const heading =
  /^\[Thread report\] Thread (\S+) finished the turn you sent it \(execution [^)]+\): ([a-z_]+)\.$/

const outcomes: ReadonlySet<string> = new Set<ThreadReportOutcome>([
  "completed",
  "failed",
  "interrupted",
  "cancelled",
  "refused",
  "output_limit",
  "request_limit",
])

/** The report a message's text is, by its first line; `null` for anything else. */
export function threadReportOf(text: string): ThreadReport | null {
  const first = text.split("\n", 1)[0] ?? ""
  const match = heading.exec(first)
  if (!match || !outcomes.has(match[2])) return null
  return { thread: match[1], outcome: match[2] as ThreadReportOutcome }
}

/** What the line says the thread did. */
export function reportVerb(outcome: ThreadReportOutcome): string {
  switch (outcome) {
    case "completed":
      return "responded"
    case "interrupted":
      return "was interrupted"
    case "cancelled":
      return "was stopped"
    case "refused":
      return "declined"
    case "output_limit":
    case "request_limit":
      return "hit its limit"
    case "failed":
      return "failed"
  }
}
