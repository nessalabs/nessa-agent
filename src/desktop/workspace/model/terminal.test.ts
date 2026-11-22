import { describe, expect, it } from "vitest"
import {
  isOverviewTerminalAgent,
  terminalAgentName,
  terminalAgentOf,
  terminalOverviewGroup,
  terminalRunsOnLabel,
  terminalStatusLine,
  type TerminalEntry,
} from "./terminal"

function makeTerminal(overrides: Partial<TerminalEntry> = {}): TerminalEntry {
  return {
    id: "t_1",
    title: "zsh",
    kind: "shell",
    channelId: "default",
    requestId: "r_1",
    folder: "/repo",
    shell: "/bin/zsh",
    runsOn: "here",
    startedAtMs: 1_000,
    state: "running",
    ...overrides,
  }
}

describe("terminal agent and OSC 7501 helpers", () => {
  it("recognizes Claude and Codex app names and ignores other programs", () => {
    expect(
      terminalAgentOf(
        makeTerminal({
          programStatus: { state: "working", app: "claude-code", updatedAtMs: 2_000 },
        }),
      ),
    ).toBe("claude")
    expect(
      terminalAgentOf(
        makeTerminal({
          programStatus: { state: "idle", app: "Claude", updatedAtMs: 2_000 },
        }),
      ),
    ).toBe("claude")
    expect(
      terminalAgentOf(
        makeTerminal({
          programStatus: { state: "working", app: "codex", updatedAtMs: 2_000 },
        }),
      ),
    ).toBe("codex")
    expect(
      terminalAgentOf(
        makeTerminal({
          programStatus: { state: "working", app: "openai-codex", updatedAtMs: 2_000 },
        }),
      ),
    ).toBe("codex")
    expect(
      terminalAgentOf(
        makeTerminal({
          programStatus: { state: "working", app: "cargo", updatedAtMs: 2_000 },
        }),
      ),
    ).toBeNull()
    expect(terminalAgentOf(makeTerminal())).toBeNull()
    expect(terminalAgentName("claude")).toBe("Claude")
    expect(terminalAgentName("codex")).toBe("Codex")
  })

  it("formats plain-language status lines from OSC 7501 fields", () => {
    expect(
      terminalStatusLine({
        state: "blocked",
        kind: "permission",
        msg: "Allow running tests?",
        updatedAtMs: 1,
      }),
    ).toBe("Allow running tests?")
    expect(
      terminalStatusLine({ state: "blocked", kind: "permission", updatedAtMs: 1 }),
    ).toBe("Needs permission")
    expect(
      terminalStatusLine({ state: "blocked", kind: "question", updatedAtMs: 1 }),
    ).toBe("Has a question")
    expect(terminalStatusLine({ state: "blocked", kind: "auth", updatedAtMs: 1 })).toBe(
      "Needs sign-in",
    )
    expect(terminalStatusLine({ state: "working", progress: 40, updatedAtMs: 1 })).toBe(
      "Working · 40%",
    )
    expect(terminalStatusLine({ state: "working", updatedAtMs: 1 })).toBe("Working")
    expect(terminalStatusLine({ state: "done", updatedAtMs: 1 })).toBe("Finished")
    expect(terminalStatusLine({ state: "error", updatedAtMs: 1 })).toBe("Failed")
  })

  it("selects overview eligibility and maps OSC 7501 states to overview groups", () => {
    const runningClaude = makeTerminal({
      programStatus: { state: "working", app: "claude-code", updatedAtMs: 2_000 },
    })
    const blockedCodex = makeTerminal({
      programStatus: {
        state: "blocked",
        kind: "permission",
        app: "codex",
        updatedAtMs: 2_000,
      },
    })
    const exitedDoneClaude = makeTerminal({
      state: "exited",
      exitStatus: 0,
      programStatus: { state: "done", app: "claude-code", updatedAtMs: 2_000 },
    })
    const exitedIdleClaude = makeTerminal({
      state: "exited",
      exitStatus: 0,
      programStatus: { state: "idle", app: "claude-code", updatedAtMs: 2_000 },
    })
    const plainShell = makeTerminal()

    expect(isOverviewTerminalAgent(runningClaude)).toBe(true)
    expect(isOverviewTerminalAgent(blockedCodex)).toBe(true)
    expect(isOverviewTerminalAgent(exitedDoneClaude)).toBe(true)
    expect(isOverviewTerminalAgent(exitedIdleClaude)).toBe(false)
    expect(isOverviewTerminalAgent(plainShell)).toBe(false)

    expect(terminalOverviewGroup(blockedCodex)).toBe("needsYou")
    expect(terminalOverviewGroup(runningClaude)).toBe("working")
    expect(terminalOverviewGroup(exitedDoneClaude)).toBe("finished")
    expect(terminalOverviewGroup(exitedIdleClaude)).toBe("earlier")
  })

  it("formats runsOn labels for local, cloud VMs, and SSH hosts without prototype lookup", () => {
    expect(terminalRunsOnLabel("here")).toBe("This Mac")
    expect(terminalRunsOnLabel("boat")).toBe("Boat VM")
    expect(terminalRunsOnLabel("agent37")).toBe("Agent37 VM")
    expect(terminalRunsOnLabel("devbox")).toBe("devbox")
    expect(terminalRunsOnLabel("toString")).toBe("toString")
  })
})
