import { describe, expect, it } from "vitest"
import { NessaConnectionClosedError } from "@nessa/client"
import type { TerminalProtocolMessage } from "../../application/ports"
import type { TerminalEntry } from "../../model/terminal"
import { settle } from "../../testing"
import { fakeGateway } from "./fake-gateway"
import { terminalGateway } from "./terminal-gateway"

/** Past every queued send: they wait in line, so a few microtasks are not enough. */
const drained = () => new Promise((resolve) => setTimeout(resolve, 30))

function setup() {
  const gateway = fakeGateway()
  let next = 0
  const source = terminalGateway({
    connected: async () => gateway.client,
    newId: () => `view-${++next}`,
  })
  return { gateway, source }
}

describe("the gateway's terminals", () => {
  it("are listed, then followed as they change, in the window's channel ids", async () => {
    const { source } = setup()
    const lists: (readonly TerminalEntry[])[] = []
    source.follow((list) => lists.push(list))
    await settle()
    expect(lists.at(-1)).toEqual([])
    const opened = await source.open({ kind: "shell" }, "r1", { cols: 80, rows: 24 })
    await settle()
    expect(lists.at(-1)?.map((terminal) => terminal.id)).toEqual([opened.id])
    expect(opened.channelId).toBe("gateway-conversations")
    await source.close(opened.id)
    await settle()
    expect(lists.at(-1)).toEqual([])
  })

  it("show a view only once it is measured, and route its output to it alone", async () => {
    const { source } = setup()
    const opened = await source.open({ kind: "shell" }, "r1", { cols: 80, rows: 24 })
    const first = source.view(opened.id)
    const second = source.view(opened.id)
    const seen: TerminalProtocolMessage[] = []
    const other: TerminalProtocolMessage[] = []
    first.subscribe((message) => seen.push(message))
    second.subscribe((message) => other.push(message))
    await settle()
    expect(seen).toEqual([])
    first.fit({ cols: 40, rows: 5 })
    await drained()
    expect(seen.map((message) => message.type)).toEqual(["ready", "frame"])
    expect(other).toEqual([])
    first.input(new TextEncoder().encode("ls\r"))
    await drained()
    const rows = (seen.at(-1)?.rows ?? []) as { cells: [string][] }[]
    expect(rows.map((row) => row.cells.map(([text]) => text).join(""))).toContain(
      "ran: ls",
    )
    first.release()
    second.release()
  })

  it("send keys in the order they were typed, though the gateway answers out of order", async () => {
    const gateway = fakeGateway()
    const arrived: string[] = []
    // Like the gateway, each input lands when its own request runs: an
    // earlier one can take longer than a later one.
    let delay = 8
    gateway.client.terminals.input = (_terminalId, bytes) =>
      new Promise((resolve) =>
        setTimeout(
          () => {
            arrived.push(new TextDecoder().decode(bytes))
            resolve()
          },
          Math.max(0, delay--),
        ),
      )
    const source = terminalGateway({
      connected: async () => gateway.client,
      newId: () => "view-1",
    })
    const opened = await source.open({ kind: "shell" }, "r1", { cols: 80, rows: 24 })
    const view = source.view(opened.id)
    for (const key of "echo s1\r") {
      view.input(new TextEncoder().encode(key))
      await new Promise((resolve) => setTimeout(resolve, 1))
    }
    await new Promise((resolve) => setTimeout(resolve, 60))
    expect(arrived.join("")).toBe("echo s1\r")
    // Keys typed while one was on its way went together.
    expect(arrived.length).toBeLessThan(8)
    view.release()
  })

  it("attach every view again when the connection comes back", async () => {
    const { gateway, source } = setup()
    const opened = await source.open({ kind: "shell" }, "r1", { cols: 80, rows: 24 })
    const view = source.view(opened.id)
    const seen: TerminalProtocolMessage[] = []
    view.subscribe((message) => seen.push(message))
    view.fit({ cols: 40, rows: 5 })
    await settle()
    const before = seen.length
    gateway.setState({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, "closed"),
    } as never)
    gateway.setState({ status: "connected" })
    await settle()
    expect(seen.length).toBeGreaterThan(before)
    expect(seen.at(-2)?.type).toBe("ready")
    view.release()
  })

  it("say why a terminal did not open", async () => {
    const { gateway, source } = setup()
    gateway.client.terminals.open = () =>
      Promise.reject(Object.assign(new Error("x"), { name: "NessaRpcError" }))
    await expect(
      source.open({ kind: "shell" }, "r", { cols: 1, rows: 1 }),
    ).rejects.toMatchObject({
      reason: "unavailable",
    })
  })
})
