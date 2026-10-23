/** A gateway tool call as a step of its message: kind, status, times on this device's clock, input, and an edit's diff. */
import { describe, expect, it } from "vitest"
import type { ConversationTool } from "@nessa/client"
import { lineDelta, toolStep } from "./gateway-steps"

const tool = (change: Partial<ConversationTool> = {}): ConversationTool => ({
  executionId: "e1",
  toolId: "t1",
  title: "Run pnpm test",
  kind: "execute",
  status: "completed",
  details: "",
  input: "",
  ...change,
})

// The gateway's clock reads 10 s ahead of this device's.
const clock = { turnEnded: false, serverNowMs: 1_000_000, receivedAtMs: 990_000 }

describe("a tool call as a step", () => {
  it("carries its times onto this device's clock, with its input and what it printed", () => {
    expect(
      toolStep(
        tool({
          startedAtMs: 999_000,
          endedAtMs: 999_400,
          input: '{"command":"pnpm test"}',
          details: "ok\n",
        }),
        clock,
      ),
    ).toEqual({
      kind: "step",
      step: "run",
      label: "Run pnpm test",
      id: "t1",
      status: "completed",
      startedAtMs: 989_000,
      endedAtMs: 989_400,
      input: '{"command":"pnpm test"}',
      output: "ok\n",
    })
  })

  it("leaves out what the gateway did not keep: times of an older call, an empty input or output", () => {
    const step = toolStep(tool(), clock)
    expect(step).toEqual({
      kind: "step",
      step: "run",
      label: "Run pnpm test",
      id: "t1",
      status: "completed",
    })
  })

  it("calls a call still at work once its turn ended stopped, and keeps any other status", () => {
    const at = (status: string, turnEnded: boolean) =>
      toolStep(tool({ status }), { ...clock, turnEnded }).status
    expect(at("pending", false)).toBe("pending")
    expect(at("running", false)).toBe("running")
    expect(at("running", true)).toBe("stopped")
    expect(at("pending", true)).toBe("stopped")
    expect(at("failed", true)).toBe("failed")
    // A status this window does not know reads as finished.
    expect(at("cancelled", false)).toBe("completed")
  })

  it("draws the first file it changed and counts lines across them all", () => {
    const step = toolStep(
      tool({
        kind: "edit",
        title: "Edit sign.sh",
        details: "Applied with a warning\nFile: ci/sign.sh\n…",
        changes: [
          { path: "ci/sign.sh", before: "a\nb", after: "a\nc\nd", cut: false },
          // Created: nothing before it.
          { path: "ci/new.sh", after: "x", cut: false },
        ],
      }),
      clock,
    )
    expect(step).toMatchObject({
      step: "edit",
      diff: { filename: "ci/sign.sh", from: "a\nb", to: "a\nc\nd" },
      added: 3,
      removed: 1,
    })
    // What else it reported beside the change stays its output.
    expect(step.output).toBe("Applied with a warning\nFile: ci/sign.sh\n…")
  })

  it("counts no lines once a side of a change was cut", () => {
    const step = toolStep(
      tool({
        kind: "edit",
        changes: [{ path: "a", before: "x", after: "y", cut: true }],
      }),
      clock,
    )
    // The trace says the diff shows only the file's start.
    expect(step.diff).toEqual({ filename: "a", from: "x", to: "y", cut: true })
    expect(step.added).toBeUndefined()
    expect(step.removed).toBeUndefined()
  })

  it("counts no lines when the list leaves out files the call changed", () => {
    const step = toolStep(
      tool({
        kind: "edit",
        changes: [{ path: "a", before: "x", after: "y", cut: false }],
        changesCut: true,
      }),
      clock,
    )
    expect(step.diff).toEqual({ filename: "a", from: "x", to: "y" })
    expect(step.added).toBeUndefined()
    expect(step.removed).toBeUndefined()
  })

  it("keeps the details of a call that changed no file as what it printed", () => {
    const step = toolStep(tool({ kind: "edit", details: "Applied 3 hunks" }), clock)
    expect(step.diff).toBeUndefined()
    expect(step.output).toBe("Applied 3 hunks")
  })
})

describe("an edit's line counts", () => {
  it("counts a moved line as neither added nor removed", () => {
    expect(lineDelta({ filename: "a", from: "x\ny\nz", to: "z\nx\ny" })).toEqual({
      added: 0,
      removed: 0,
    })
    expect(lineDelta({ filename: "a", from: "", to: "x\ny" })).toEqual({
      added: 2,
      removed: 0,
    })
    // The closing newline is not a line; a blank line before it is.
    expect(lineDelta({ filename: "a", from: "", to: "x\n" })).toEqual({
      added: 1,
      removed: 0,
    })
    expect(lineDelta({ filename: "a", from: "x\n\n", to: "" })).toEqual({
      added: 0,
      removed: 2,
    })
  })
})
