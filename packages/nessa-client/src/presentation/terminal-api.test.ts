import { describe, expect, it, vi } from "vitest"
import type { TerminalSummary } from "../generated/product.js"
import { createTerminalApi, terminalInputLimit } from "./terminal-api.js"

const shell: TerminalSummary = {
  terminalId: `t_${"a".repeat(32)}`,
  title: "zsh",
  kind: "shell",
  requestId: "r1",
  folder: "/Users/me/project",
  shell: "/bin/zsh",
  runsOn: "here",
  startedAtMs: 1000,
  state: "running",
}

function requester(answer: unknown = {}) {
  const request = vi.fn(async (_method: string, _params: unknown) => answer)
  return { request, api: createTerminalApi({ request } as never) }
}

describe("terminal api", () => {
  it("opens and lists, checking the answer", async () => {
    const opened = requester({ terminal: shell })
    await expect(
      opened.api.open({ requestId: "r1", cols: 80, rows: 24 }),
    ).resolves.toEqual(shell)
    expect(opened.request).toHaveBeenCalledWith("terminal.open", {
      requestId: "r1",
      cols: 80,
      rows: 24,
    })
    await expect(
      requester({ terminal: { title: 1 } }).api.open({
        requestId: "r",
        cols: 1,
        rows: 1,
      }),
    ).rejects.toThrow()
    await expect(requester({ terminals: [shell] }).api.list()).resolves.toEqual([shell])
  })

  it("sends input as base64, in pieces the gateway takes", async () => {
    const { api, request } = requester()
    await api.input(shell.terminalId, new TextEncoder().encode("ls\r"))
    expect(request).toHaveBeenCalledWith("terminal.input", {
      terminalId: shell.terminalId,
      data: "bHMN",
    })
    request.mockClear()
    await api.input(shell.terminalId, new Uint8Array(terminalInputLimit + 1))
    expect(request).toHaveBeenCalledTimes(2)
    const first = (request.mock.calls[0]![1] as { data: string }).data
    expect(first.length).toBeLessThanOrEqual(43692)
  })

  it("names the view for attach, history and detach", async () => {
    const { api, request } = requester()
    await api.attach({ terminalId: shell.terminalId, viewId: "v", cols: 80, rows: 24 })
    await api.history(shell.terminalId, "v", 10, 24)
    await api.detach(shell.terminalId, "v")
    expect(request.mock.calls.map(([method]) => method)).toEqual([
      "terminal.attach",
      "terminal.history",
      "terminal.detach",
    ])
  })
})
