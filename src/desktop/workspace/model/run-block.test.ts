import { describe, expect, it } from "vitest"
import type { TerminalEntry } from "./terminal"
import {
  durationWords,
  latestRun,
  runCommand,
  runEndWords,
  runLimitBytes,
  runnableFence,
  runRequestBase,
  runRequestId,
} from "./run-block"

const run = (over: Partial<TerminalEntry>): TerminalEntry => ({
  id: "t1",
  title: "ls",
  kind: "run",
  channelId: "gateway-conversations",
  conversationId: "c",
  requestId: "x",
  folder: "/w",
  shell: "/bin/zsh",
  runsOn: "here",
  startedAtMs: 1000,
  state: "running",
  ...over,
})

describe("a shell fence", () => {
  it("runs for shell words only", () => {
    for (const word of ["bash", "sh", "shell", "zsh", "console", "BASH"])
      expect(runnableFence(word)).toBe(true)
    for (const word of [undefined, "ts", "python", "text"])
      expect(runnableFence(word)).toBe(false)
  })

  it("runs a console fence's $ lines only", () => {
    expect(runCommand("$ ls\nfile.txt\n$ pwd\n/w\n", "console")).toEqual({
      command: "ls\npwd",
    })
    expect(runCommand("file.txt\n", "console")).toEqual({ refused: "empty" })
  })

  it("runs the rest whole, trailing space dropped", () => {
    expect(runCommand("echo hi\necho there\n\n", "bash")).toEqual({
      command: "echo hi\necho there",
    })
  })

  it("refuses past the limit in bytes, not characters", () => {
    expect(runCommand("é".repeat(runLimitBytes / 2), "sh")).toHaveProperty("command")
    expect(runCommand("é".repeat(runLimitBytes / 2 + 1), "sh")).toEqual({
      refused: "too-long",
    })
  })
})

describe("a block's runs", () => {
  const base = runRequestBase("m1", 2, "3")
  it("finds the newest run of this block in this conversation only", () => {
    const terminals = {
      a: run({ id: "a", requestId: runRequestId(base, "1"), startedAtMs: 1 }),
      b: run({ id: "b", requestId: runRequestId(base, "2"), startedAtMs: 5 }),
      c: run({
        id: "c",
        requestId: runRequestId(runRequestBase("m1", 2, "31"), "1"),
        startedAtMs: 9,
      }),
      d: run({
        id: "d",
        requestId: runRequestId(base, "3"),
        startedAtMs: 9,
        conversationId: "o",
      }),
      e: run({
        id: "e",
        requestId: runRequestId(base, "4"),
        startedAtMs: 9,
        kind: "shell",
      }),
    }
    expect(latestRun(terminals, "c", base)?.id).toBe("b")
    expect(latestRun({}, "c", base)).toBeUndefined()
  })

  it("says how a run ended", () => {
    expect(runEndWords(run({}))).toBeUndefined()
    expect(runEndWords(run({ state: "exited", exitStatus: 0, endedAtMs: 2200 }))).toBe(
      "exit 0 · 1.2s",
    )
    expect(runEndWords(run({ state: "exited" }))).toBe("ended")
    expect(durationWords(42_000)).toBe("42s")
    expect(durationWords(185_000)).toBe("3m 5s")
  })
})
