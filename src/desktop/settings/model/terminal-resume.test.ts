import { describe, expect, it } from "vitest"
import {
  describeRule,
  draftRule,
  refusalMessage,
  savedProgramDetail,
} from "./terminal-resume"

describe("terminal resume rules", () => {
  it("keeps a rule as typed, trimmed, with only what was given", () => {
    expect(
      draftRule({ program: " claude ", folder: "", args: "", action: "allow" }),
    ).toEqual({ program: "claude", action: "allow" })
    expect(
      draftRule({
        program: "/opt/homebrew/bin/nvim",
        folder: "/Users/you/code ",
        args: "  -S   session.vim ",
        action: "never",
      }),
    ).toEqual({
      program: "/opt/homebrew/bin/nvim",
      action: "never",
      folder: "/Users/you/code",
      args: ["-S", "session.vim"],
    })
  })

  it("says which part to fix for each refusal the gateway gives", () => {
    for (const part of ["program", "folder", "args", "count"] as const)
      expect(refusalMessage(part)).not.toBe("")
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
