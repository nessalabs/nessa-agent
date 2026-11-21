import { describe, expect, it } from "vitest"
import { makeDesktopStore } from "../../../store"
import { createQuestionFlights } from "../../application/question-flights"
import type { TerminalSource, TerminalView } from "../../application/ports"
import type { TerminalEntry } from "../../model/terminal"
import { fakeSource, keptFilter, roomyGrid, settle, shownIn } from "../../testing"
import {
  closeTerminal,
  interruptTerminal,
  loadWorkspace,
  newTerminal,
  runInConversation,
} from "./commands"

const id = (n: number) => `t_${n.toString(16).padStart(32, "0")}`

/** Terminals kept in memory, telling each follower the whole list on each change. */
function memoryTerminals() {
  const list: TerminalEntry[] = []
  const followers = new Set<(terminals: readonly TerminalEntry[]) => void>()
  const tell = () => followers.forEach((follower) => follower([...list]))
  const opened: unknown[] = []
  const typed: Array<[string, number[]]> = []
  const source: TerminalSource = {
    follow(onList) {
      followers.add(onList)
      onList([...list])
      return () => followers.delete(onList)
    },
    async open(request, requestId) {
      opened.push(request)
      const entry: TerminalEntry = {
        id: id(list.length + 1),
        title: request.kind === "run" ? request.command : "zsh",
        kind: request.kind,
        ...(request.kind === "run" ? { conversationId: request.conversationId } : {}),
        channelId:
          request.kind === "shell" ? (request.channelId ?? "gateway-conversations") : "x",
        requestId,
        folder: "/tmp",
        shell: "/bin/zsh",
        runsOn: "here",
        startedAtMs: 1,
        state: "running",
      }
      list.push(entry)
      return entry
    },
    view: () => ({}) as TerminalView,
    input(terminalId, bytes) {
      typed.push([terminalId, [...bytes]])
    },
    async close(terminalId) {
      list.splice(
        list.findIndex((terminal) => terminal.id === terminalId),
        1,
      )
      tell()
    },
  }
  return { source, opened, list, typed }
}

async function store(terminals?: TerminalSource) {
  let next = 0
  const made = makeDesktopStore({
    workspace: fakeSource(),
    now: () => 1000,
    newId: () => `id-${++next}`,
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    questionFlights: createQuestionFlights(),
    terminals,
  })
  await made.dispatch(loadWorkspace())
  await settle()
  return made
}

describe("a new terminal", () => {
  it("opens in the focused pane, in the channel in front, and is listed", async () => {
    const memory = memoryTerminals()
    const made = await store(memory.source)
    const terminalId = await made.dispatch(newTerminal())
    expect(terminalId).toBe(id(1))
    expect(made.getState().workspace.terminals[id(1)]?.title).toBe("zsh")
    expect(shownIn(made.getState().workspace.panes)).toContain(`terminal ${id(1)}`)
    expect(memory.opened).toEqual([{ kind: "shell", channelId: expect.any(String) }])
  })

  it("asks for the channel it is given", async () => {
    const memory = memoryTerminals()
    const made = await store(memory.source)
    await made.dispatch(newTerminal({ channelId: "ops" }))
    expect(memory.opened).toEqual([{ kind: "shell", channelId: "ops" }])
  })

  it("is nothing where the window has no terminals", async () => {
    const made = await store()
    const panes = made.getState().workspace.panes
    await expect(made.dispatch(newTerminal())).resolves.toBeUndefined()
    expect(made.getState().workspace.panes).toBe(panes)
  })
})

describe("ending a terminal", () => {
  it("closes its pane and takes it off the list", async () => {
    const memory = memoryTerminals()
    const made = await store(memory.source)
    await made.dispatch(newTerminal())
    await made.dispatch(closeTerminal(id(1)))
    expect(shownIn(made.getState().workspace.panes)).not.toContain(`terminal ${id(1)}`)
    expect(made.getState().workspace.terminals).toEqual({})
  })
})

describe("a Run", () => {
  it("opens a run in the conversation, lists it at once and opens no pane", async () => {
    const memory = memoryTerminals()
    const made = await store(memory.source)
    const before = shownIn(made.getState().workspace.panes)
    const entry = await made.dispatch(
      runInConversation({
        conversationId: "c1",
        command: "ls -la",
        requestId: "run m 0 0 #a",
        fit: { cols: 80, rows: 12 },
      }),
    )
    expect(memory.opened).toEqual([
      { kind: "run", conversationId: "c1", command: "ls -la" },
    ])
    expect(entry?.requestId).toBe("run m 0 0 #a")
    expect(made.getState().workspace.terminals[id(1)]?.kind).toBe("run")
    expect(shownIn(made.getState().workspace.panes)).toEqual(before)
  })

  it("stops with Control-C", async () => {
    const memory = memoryTerminals()
    const made = await store(memory.source)
    made.dispatch(interruptTerminal(id(7)))
    expect(memory.typed).toEqual([[id(7), [3]]])
  })
})
