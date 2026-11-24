import { describe, expect, it } from "vitest"
import { unsentLabel } from "../ui/failure-copy"
import { atWork, emptyTranscript } from "./transcript"
import {
  SETUP_WORTH_A_LINE_MS,
  runningSetup,
  setupFailedFor,
  setupReadyLabel,
  setupStepLabel,
  setupTookMs,
  type MachineSetup,
} from "./machine-setup"

const ready: MachineSetup = {
  state: "ready",
  executionId: "m1",
  steps: [
    { step: "creating", startedAtMs: 1_000, endedAtMs: 19_000 },
    { step: "connecting", startedAtMs: 19_000, endedAtMs: 23_000 },
    { step: "starting", startedAtMs: 23_000, endedAtMs: 40_400 },
  ],
}

describe("a cloud machine's setup", () => {
  it("says each step in a person's words, naming the chat's agent when it is known", () => {
    expect(setupStepLabel("creating")).toBe("Setting up container")
    expect(setupStepLabel("waking")).toBe("Waking container")
    expect(setupStepLabel("installing")).toBe("Installing Nessa")
    expect(setupStepLabel("connecting")).toBe("Connecting")
    expect(setupStepLabel("starting", "codex")).toBe("Starting Codex")
    expect(setupStepLabel("starting")).toBe("Starting agent")
  })

  it("times a finished one from its first step to its last, and says so in whole seconds", () => {
    expect(setupTookMs(ready)).toBe(39_400)
    expect(setupReadyLabel(ready)).toBe("Container ready in 39s")
    expect(setupTookMs({ state: "ready", steps: [] })).toBe(0)
    expect(SETUP_WORTH_A_LINE_MS).toBeGreaterThan(0)
  })

  it("finds the one under way, and whether a message's own opening failed", () => {
    const failed: MachineSetup = { ...ready, state: "failed" }
    const running: MachineSetup = { state: "running", steps: ready.steps.slice(0, 1) }
    expect(runningSetup([failed, running])).toBe(running)
    expect(runningSetup([{ state: "running", steps: [] }])).toBeUndefined()
    expect(setupFailedFor([failed], "m1")).toBe(true)
    expect(setupFailedFor([failed], "m2")).toBe(false)
    expect(setupFailedFor(undefined, "m1")).toBe(false)
  })
})

describe("why a message did not go, in two or three words", () => {
  it("says the container when its machine never came up, else the cause a person acts on", () => {
    expect(unsentLabel("unavailable", "environment_unreachable", true)).toBe(
      "Container didn’t start",
    )
    expect(unsentLabel("unavailable", "agent_sign_in_required", false)).toBe(
      "Not signed in",
    )
    expect(unsentLabel("unavailable", "environment_not_configured", false)).toBe(
      "Boat not connected",
    )
    expect(unsentLabel("unavailable", "environment_unreachable", false)).toBe(
      "Machine unreachable",
    )
    expect(unsentLabel("agent-not-configured", undefined, false)).toBe(
      "Agent needs setup",
    )
    expect(unsentLabel("not-supported", "model_unavailable", false)).toBe("Couldn’t send")
    for (const label of [
      unsentLabel("unavailable", undefined, false),
      unsentLabel("unavailable", undefined, true),
    ])
      expect(label.split(" ").length).toBeLessThanOrEqual(3)
  })
})

describe("a chat whose machine is being made ready", () => {
  it("is not at work: no turn to stop yet, so Send stays Send", () => {
    const setting = {
      ...emptyTranscript("s"),
      activity: { label: "Waking container", since: 1, setup: true as const },
    }
    expect(atWork(setting)).toBe(false)
    expect(atWork({ ...setting, activity: { label: "Thinking", since: 1 } })).toBe(true)
  })
})
