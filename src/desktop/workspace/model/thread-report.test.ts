import { readFileSync } from "node:fs"
import { expect, it } from "vitest"
import { reportVerb, threadReportOf } from "./thread-report"

// The gateway's own cases: its test writes these headings, this one reads them.
const shared = JSON.parse(
  readFileSync(
    new URL(
      "../../../../crates/nessa-server/tests/fixtures/thread-report.json",
      import.meta.url,
    ),
    "utf8",
  ),
) as { cases: { child: string; outcome: string; reply: string | null; header: string }[] }

it("reads the gateway's heading: which thread, and how its turn ended", () => {
  for (const case_ of shared.cases) {
    const text = case_.reply
      ? `${case_.header}\n\nIts reply:\n${case_.reply}`
      : `${case_.header}\n\nThe gateway restarted before the turn finished.`
    expect(threadReportOf(text)).toEqual({ thread: case_.child, outcome: case_.outcome })
  }
})

it("reads nothing else as a report, a reply that quotes one included", () => {
  expect(threadReportOf("Hello")).toBeNull()
  expect(threadReportOf(`Look:\n${shared.cases[0].header}`)).toBeNull()
  expect(
    threadReportOf(shared.cases[0].header.replace("completed", "exploded")),
  ).toBeNull()
})

it("says what the thread did in a few words", () => {
  expect(reportVerb("completed")).toBe("responded")
  expect(reportVerb("failed")).toBe("failed")
  expect(reportVerb("interrupted")).toBe("was interrupted")
})
