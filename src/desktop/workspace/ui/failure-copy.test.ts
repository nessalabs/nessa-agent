import { expect, it } from "vitest"
import type { WorkspaceFailureReason } from "../model/failure"
import { failureCopy, readFailureCopy } from "./failure-copy"

// Every reason, from a total record: a reason added to the model does not
// compile until it is listed here, so neither table can leave one out.
const every: Record<WorkspaceFailureReason, true> = {
  unavailable: true,
  "unknown-session": true,
  "not-waiting": true,
  "not-supported": true,
  "signed-out": true,
  "not-started": true,
  "not-ready": true,
  "not-listening": true,
  "wrong-stage": true,
}
const reasons = Object.keys(every) as WorkspaceFailureReason[]

it("says each reason in words of its own", () => {
  for (const copy of [failureCopy, readFailureCopy]) {
    const said = reasons.map((reason) => copy(reason))
    expect(said.every((sentence) => sentence.length > 0)).toBe(true)
    expect(new Set(said).size).toBe(reasons.length)
  }
})

it("a read says what it could not read only where no answer came, and otherwise what any call says", () => {
  for (const reason of reasons)
    if (reason === "unavailable")
      expect(readFailureCopy(reason)).not.toBe(failureCopy(reason))
    else expect(readFailureCopy(reason)).toBe(failureCopy(reason))
})
