import { expect, it } from "vitest"
import type { WorkspaceFailureReason } from "../model/failure"
import { failureCopy } from "./failure-copy"

it("says each reason in words of its own", () => {
  const reasons: WorkspaceFailureReason[] = [
    "unavailable",
    "unknown-session",
    "not-waiting",
  ]
  const said = reasons.map(failureCopy)
  expect(said.every((sentence) => sentence.length > 0)).toBe(true)
  expect(new Set(said).size).toBe(reasons.length)
})
