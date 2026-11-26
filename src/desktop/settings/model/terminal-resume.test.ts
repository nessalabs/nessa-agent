import { describe, expect, it } from "vitest"
import { describeRule, parseRule, savedProgramDetail } from "./terminal-resume"

describe("terminal resume rules", () => {
  it("keeps a rule as typed, trimmed, with only what was given", () => {
    expect(
      parseRule({ program: " claude ", folder: "", args: "", action: "allow" }),
    ).toEqual({ rule: { program: "claude", action: "allow" } })
    expect(
      parseRule({
        program: "/opt/homebrew/bin/nvim",
        folder: "/Users/you/code ",
        args: "  -S   session.vim ",
        action: "never",
      }),
    ).toEqual({
      rule: {
        program: "/opt/homebrew/bin/nvim",
        action: "never",
        folder: "/Users/you/code",
        args: ["-S", "session.vim"],
      },
    })
  })

  it("refuses what the gateway would refuse", () => {
    for (const draft of [
      { program: "  ", folder: "", args: "", action: "allow" as const },
      { program: "bin/claude", folder: "", args: "", action: "allow" as const },
      { program: "claude", folder: "code", args: "", action: "allow" as const },
      {
        program: "claude",
        folder: "",
        args: Array.from({ length: 17 }, (_, i) => `-${i}`).join(" "),
        action: "allow" as const,
      },
    ])
      expect(parseRule(draft)).toHaveProperty("problem")
  })

  it("says what a rule does and whether a kept program comes back", () => {
    expect(describeRule({ program: "*", action: "allow" })).toBe("Allow every program")
    expect(
      describeRule({
        program: "codex",
        action: "never",
        folder: "/work",
        args: ["--yolo"],
      }),
    ).toBe("Never · in /work · with --yolo")
    const saved = {
      terminalId: "t_1",
      title: "zsh",
      command: "nvim -S /tmp/s.vim",
      savedAtMs: 1,
      open: true,
    }
    expect(savedProgramDetail({ ...saved, comesBack: true })).toBe(
      "nvim -S /tmp/s.vim · Comes back after a restart",
    )
    expect(savedProgramDetail({ ...saved, comesBack: false })).toContain(
      "Won’t come back",
    )
  })
})
