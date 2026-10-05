import { expect, it } from "vitest"
import type { WorkspaceFailureReason } from "../model/failure"
import { failureCopy, readFailureCopy, type ReadSubject } from "./failure-copy"

// Every reason, from a total record: a reason added to the model does not
// compile until it is listed here, so each table is checked for every reason.
const every: Record<WorkspaceFailureReason, true> = {
  unavailable: true,
  "unknown-session": true,
  "not-waiting": true,
  "not-supported": true,
  "signed-out": true,
}
const reasons = Object.keys(every) as WorkspaceFailureReason[]

const reads: Record<ReadSubject, true> = { index: true, conversation: true }
const subjects = Object.keys(reads) as ReadSubject[]

it("says each reason in words of its own", () => {
  const tables = [
    failureCopy,
    ...subjects.map(
      (read) => (reason: WorkspaceFailureReason) => readFailureCopy(reason, read),
    ),
  ]
  for (const copy of tables) {
    const said = reasons.map(copy)
    expect(said.every((sentence) => sentence.length > 0)).toBe(true)
    expect(new Set(said).size).toBe(reasons.length)
  }
})

it("a read says what it could not read only where no answer came, and otherwise what any call says", () => {
  for (const read of subjects)
    for (const reason of reasons)
      if (reason === "unavailable")
        expect(readFailureCopy(reason, read)).not.toBe(failureCopy(reason))
      else expect(readFailureCopy(reason, read)).toBe(failureCopy(reason))
  expect(readFailureCopy("unavailable", "index")).not.toBe(
    readFailureCopy("unavailable", "conversation"),
  )
})

it("names the index, and a conversation, when no answer came", () => {
  expect(readFailureCopy("unavailable", "index")).toBe(
    "Nessa couldn’t read the local server’s conversations just now.",
  )
  expect(readFailureCopy("unavailable", "conversation")).toBe(
    "Nessa couldn’t read this conversation just now.",
  )
})
