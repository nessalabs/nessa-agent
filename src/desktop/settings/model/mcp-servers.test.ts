/**
 * Settings › Integrations' reducer, one row of the design's state table
 * (#391 PR 3, U1–U31) at least one test, each named for its row. Rows the
 * reducer cannot see — the pending row with no gateway (U1), skeletons (U3),
 * widths (U29, U30) and the DOM (U31) — are `integrations-tab.test.tsx`'s and
 * the browser check's (`verification/desktop/scripts/mcp-servers-gateway.mjs`).
 */
import { describe, expect, it } from "vitest"
import {
  argsOf,
  canInspect,
  canWrite,
  formReady,
  initialMcpServersState,
  mcpServersReducer,
  sentences,
  type Failure,
  type Inspection,
  type ListedServer,
  type McpServersEvent,
  type McpServersState,
  type Outcome,
  type ServerList,
} from "./mcp-servers"

const limits = { inspectDeadlineMs: 30_000 }
const charts: ListedServer = {
  name: "charts",
  command: "/usr/bin/node",
  args: ["server.mjs", "--port", "1"],
  envNames: ["TOKEN"],
  enabled: true,
  managed: false,
}
const nessa: ListedServer = {
  name: "nessa",
  command: "/Applications/Nessa",
  args: ["mcp"],
  envNames: [],
  enabled: true,
  managed: true,
}
const listOf = (...servers: ListedServer[]): ServerList => ({ revision: "r1", servers })

function run(state: McpServersState, ...events: McpServersEvent[]) {
  return events.reduce(mcpServersReducer, state)
}

const ok = <T>(value: T): Outcome<T> => ({ ok: true, value })
const no = (failure: Failure): Outcome<never> => ({ ok: false, failure })

/** Answers the request in flight. */
function answer(state: McpServersState, outcome: Outcome<unknown>) {
  if (!state.pending) throw new Error("nothing in flight")
  return mcpServersReducer(state, { type: "answered", seq: state.pending.seq, outcome })
}

/** Connected as an administrator, the list answered. */
function listed(list: ServerList = listOf(charts, nessa)) {
  return answer(
    run(initialMcpServersState(limits), { type: "connected", mayManage: true }),
    ok(list),
  )
}

describe("access", () => {
  it("U2: without the grant, the admin notice and no request", () => {
    const state = run(initialMcpServersState(limits), {
      type: "connected",
      mayManage: false,
    })
    expect(state.access).toBe("notAdmin")
    expect(state.pending).toBeNull()
    expect(canWrite(state)).toBe(false)
    expect(run(state, { type: "retry" }).pending).toBeNull()
    expect(run(state, { type: "add" }).form).toBeNull()
  })

  it("U20: forbidden mid-session is U2, whatever was open", () => {
    let state = run(listed(), { type: "edit", name: "charts" })
    state = run(state, { type: "save" })
    state = answer(state, no({ kind: "forbidden" }))
    expect(state.access).toBe("notAdmin")
    expect(state.form).toBeNull()
    expect(state.pending).toBeNull()
    const inspecting = run(listed(), { type: "inspect", name: "charts" })
    const seq =
      inspecting.inspection?.phase === "running" ? inspecting.inspection.seq : -1
    const after = run(inspecting, {
      type: "inspected",
      seq,
      outcome: no({ kind: "forbidden" }),
    })
    expect(after.access).toBe("notAdmin")
    expect(after.inspection).toBeNull()
  })
})

describe("listing", () => {
  it("U3: listing on connection, Add not offered until listed", () => {
    const state = run(initialMcpServersState(limits), {
      type: "connected",
      mayManage: true,
    })
    expect(state.pending).toEqual({ kind: "list", seq: 1 })
    expect(state.list.phase).toBe("loading")
    expect(canWrite(state)).toBe(false)
    expect(run(state, { type: "add" }).form).toBeNull()
  })

  it("U4: not configured says so and offers no Add", () => {
    const state = answer(
      run(initialMcpServersState(limits), { type: "connected", mayManage: true }),
      no({ kind: "notConfigured" }),
    )
    expect(state.list.phase).toBe("notConfigured")
    expect(canWrite(state)).toBe(false)
    expect(run(state, { type: "add" }).form).toBeNull()
  })

  it("U5: only the managed server is an empty list, Add offered", () => {
    const state = listed(listOf(nessa))
    expect(state.list).toEqual({ phase: "listed", list: listOf(nessa) })
    expect(canWrite(state)).toBe(true)
  })

  it("U6: the list is the gateway's, in its order", () => {
    const state = listed(listOf(charts, nessa))
    expect(state.list.phase === "listed" && state.list.list.servers).toEqual([
      charts,
      nessa,
    ])
    expect(sentences.variables(1)).toBe("1 variable")
    expect(sentences.variables(2)).toBe("2 variables")
  })

  it("an answer for a request since replaced changes nothing", () => {
    const state = run(initialMcpServersState(limits), {
      type: "connected",
      mayManage: true,
    })
    expect(run(state, { type: "answered", seq: 99, outcome: ok(listOf(charts)) })).toBe(
      state,
    )
  })

  it("a list that succeeds after a reconnect clears the failed list's notice", () => {
    // The list fails, the connection drops and returns, the list succeeds:
    // the failure is answered, and its notice goes.
    let state = answer(
      run(initialMcpServersState(limits), { type: "connected", mayManage: true }),
      no({ kind: "configInvalid" }),
    )
    expect(state.notice).toEqual({ text: sentences.configInvalid, from: "list" })
    state = run(state, { type: "unreachable" }, { type: "connected", mayManage: true })
    expect(state.notice?.text).toBe(sentences.configInvalid)
    state = answer(state, ok(listOf(charts, nessa)))
    expect(state.list.phase).toBe("listed")
    expect(state.notice).toBeNull()
  })

  it("a write not confirmed keeps its sentence through the lists after it, until the next action", () => {
    let state = run(listed(), { type: "toggle", name: "charts" })
    state = answer(state, no({ kind: "unanswered" }))
    expect(state.notice).toEqual({ text: sentences.unanswered, from: "write" })
    state = answer(state, ok(listOf(charts, nessa)))
    expect(state.notice?.text).toBe(sentences.unanswered)
    state = run(state, { type: "unreachable" }, { type: "connected", mayManage: true })
    state = answer(state, ok(listOf(charts, nessa)))
    expect(state.notice?.text).toBe(sentences.unanswered)
    expect(run(state, { type: "add" }).notice).toBeNull()
  })

  it("a list that fails says so and lists again only when asked", () => {
    let state = answer(
      run(initialMcpServersState(limits), { type: "connected", mayManage: true }),
      no({ kind: "unanswered" }),
    )
    expect(state.list.phase).toBe("failed")
    expect(state.notice?.text).toBe(sentences.listFailed)
    expect(state.pending).toBeNull()
    state = run(state, { type: "retry" })
    expect(state.pending?.kind).toBe("list")
  })
})

describe("the form", () => {
  it("U7: Add opens an empty form, on, not ready until name and command are non-blank", () => {
    let state = run(listed(), { type: "add" })
    expect(state.form).toEqual({
      name: "",
      command: "",
      args: "",
      env: [],
      enabled: true,
    })
    expect(formReady(state.form!)).toBe(false)
    expect(run(state, { type: "save" }).pending).toBeNull()
    state = run(state, { type: "change", patch: { name: "x" } })
    expect(formReady(state.form!)).toBe(false)
    state = run(state, { type: "change", patch: { command: "/bin/x" } })
    expect(formReady(state.form!)).toBe(true)
  })

  it("U8: a saved server closes the form and lists again", () => {
    let state = run(
      listed(),
      { type: "add" },
      { type: "change", patch: { name: "maps", command: "/bin/maps", args: "a\nb\n" } },
      { type: "addVariable" },
    )
    const key = state.form!.env[0].key
    state = run(
      state,
      { type: "changeVariable", key, patch: { name: "KEY", value: "s3cret" } },
      { type: "addVariable" },
      { type: "save" },
    )
    expect(state.pending).toMatchObject({
      kind: "save",
      request: {
        revision: "r1",
        server: {
          name: "maps",
          command: "/bin/maps",
          args: ["a", "b"],
          // The blank row is not sent.
          env: [{ name: "KEY", value: "s3cret" }],
          enabled: true,
        },
      },
    })
    expect(state.pending).not.toHaveProperty("request.previousName")
    state = answer(state, ok(undefined))
    expect(state.form).toBeNull()
    expect(state.pending?.kind).toBe("list")
  })

  it("U9: invalid shows its problem at its field and keeps the form, listing nothing", () => {
    let state = run(
      listed(),
      { type: "add" },
      { type: "change", patch: { name: "x y", command: "/bin/x" } },
      { type: "save" },
    )
    state = answer(state, no({ kind: "invalid", problem: "name" }))
    expect(state.form?.problem?.field).toBe("name")
    expect(state.form?.name).toBe("x y")
    expect(state.pending).toBeNull()
    const variable = answer(
      run(state, { type: "save" }),
      no({ kind: "invalid", problem: "environmentValueMissing", name: "KEY" }),
    )
    expect(variable.form?.problem).toEqual({
      field: "env",
      text: "“KEY” has no stored value. Enter one.",
    })
    const many = answer(
      run(state, { type: "save" }),
      no({ kind: "invalid", problem: "tooMany" }),
    )
    expect(many.form?.problem?.field).toBe("form")
    // The next save clears the problem it is about to be judged again on.
    expect(run(variable, { type: "save" }).form?.problem).toBeUndefined()
  })

  it("U9: a problem with another stored server names it and keeps the form clear of it", () => {
    const state = run(
      listed(),
      { type: "add" },
      { type: "change", patch: { name: "maps", command: "/bin/maps" } },
      { type: "save" },
    )
    const own = answer(
      state,
      no({ kind: "invalid", problem: "duplicateName", server: "maps" }),
    )
    expect(own.form?.problem).toEqual({
      field: "name",
      text: "Another server is named “maps”.",
    })
    const other = answer(
      state,
      no({ kind: "invalid", problem: "command", server: "hand" }),
    )
    expect(other.form?.problem).toBeUndefined()
    expect(other.form?.name).toBe("maps")
    expect(other.notice?.text).toBe("“hand”: This command can't be used.")
    expect(other.pending).toBeNull()
  })

  it("U10: Edit fills the form, every stored value kept unless typed", () => {
    let state = run(listed(), { type: "edit", name: "charts" })
    expect(state.form).toMatchObject({
      editing: "charts",
      name: "charts",
      command: "/usr/bin/node",
      args: "server.mjs\n--port\n1",
      env: [{ name: "TOKEN", value: "", stored: true }],
      enabled: true,
    })
    state = run(state, { type: "save" })
    expect(state.pending).toMatchObject({
      request: { server: { env: [{ name: "TOKEN", value: null }] } },
    })
    // A stored variable's name stays the stored one; typing a value replaces it.
    const typed = run(listed(), { type: "edit", name: "charts" })
    const key = typed.form!.env[0].key
    const changed = run(
      typed,
      { type: "changeVariable", key, patch: { name: "OTHER", value: "new" } },
      { type: "save" },
    )
    expect(changed.pending).toMatchObject({
      request: { server: { env: [{ name: "TOKEN", value: "new" }] } },
    })
    // Deleting the row removes the variable.
    const removed = run(typed, { type: "removeVariable", key }, { type: "save" })
    expect(removed.pending).toMatchObject({ request: { server: { env: [] } } })
  })

  it("U10: the managed server is never edited", () => {
    expect(run(listed(), { type: "edit", name: "nessa" }).form).toBeNull()
  })

  it("U11: a new name sends the stored one as previousName", () => {
    const state = run(
      listed(),
      { type: "edit", name: "charts" },
      { type: "change", patch: { name: "graphs" } },
      { type: "save" },
    )
    expect(state.pending).toMatchObject({
      request: { previousName: "charts", server: { name: "graphs" } },
    })
  })

  it("arguments are one per line, a final line break no argument of its own", () => {
    expect(argsOf("")).toEqual([])
    expect(argsOf("a\n")).toEqual(["a"])
    expect(argsOf("a\n\nb")).toEqual(["a", "", "b"])
  })
})

describe("the switch", () => {
  it("U12: sends the listed server, every value kept, and shows the new list", () => {
    let state = run(listed(), { type: "toggle", name: "charts" })
    expect(state.pending).toEqual({
      kind: "save",
      seq: 2,
      toggled: "charts",
      request: {
        revision: "r1",
        server: {
          name: "charts",
          command: "/usr/bin/node",
          args: ["server.mjs", "--port", "1"],
          env: [{ name: "TOKEN", value: null }],
          enabled: false,
        },
      },
    })
    expect(canWrite(state)).toBe(false)
    // Nothing optimistic: the switch shows what the list said until it says otherwise.
    expect(state.list.phase === "listed" && state.list.list.servers[0].enabled).toBe(true)
    state = answer(state, ok(undefined))
    expect(state.pending?.kind).toBe("list")
    state = answer(state, ok(listOf({ ...charts, enabled: false }, nessa)))
    expect(state.list.phase === "listed" && state.list.list.servers[0].enabled).toBe(
      false,
    )
  })

  it("U27: the managed server's switch sends nothing", () => {
    const state = listed()
    expect(run(state, { type: "toggle", name: "nessa" })).toBe(state)
    expect(run(state, { type: "askRemove", name: "nessa" }).confirming).toBeNull()
    expect(run(state, { type: "inspect", name: "nessa" }).inspection).toBeNull()
  })
})

describe("removing", () => {
  it("U13: confirmed, the removal is sent and the list read again", () => {
    let state = run(listed(), { type: "askRemove", name: "charts" })
    expect(state.confirming).toBe("charts")
    expect(state.pending).toBeNull()
    state = run(state, { type: "confirmRemove" })
    expect(state.pending).toMatchObject({
      kind: "remove",
      request: { revision: "r1", name: "charts" },
    })
    state = answer(state, ok(undefined))
    expect(state.confirming).toBeNull()
    expect(state.pending?.kind).toBe("list")
    state = answer(state, ok(listOf(nessa)))
    expect(state.list.phase === "listed" && state.list.list.servers).toEqual([nessa])
  })

  it("U14: cancelled, nothing is sent", () => {
    const state = run(
      listed(),
      { type: "askRemove", name: "charts" },
      { type: "cancelRemove" },
    )
    expect(state.confirming).toBeNull()
    expect(state.pending).toBeNull()
    expect(sentences.removeAsk("charts")).toBe(
      "Remove “charts”? New conversations stop getting it. Open ones keep it until they close.",
    )
  })
})

describe("refusals", () => {
  const saving = () =>
    run(
      listed(),
      { type: "edit", name: "charts" },
      { type: "change", patch: { command: "/bin/new" } },
      { type: "save" },
    )

  it("U15: a conflict reloads, says so, and keeps what was typed", () => {
    const state = answer(saving(), no({ kind: "revisionConflict" }))
    expect(state.notice?.text).toBe(sentences.conflict)
    expect(state.form?.command).toBe("/bin/new")
    expect(state.pending?.kind).toBe("list")
  })

  it("U16: not found says so and reloads", () => {
    const state = answer(
      run(listed(), { type: "askRemove", name: "charts" }, { type: "confirmRemove" }),
      no({ kind: "notFound" }),
    )
    expect(state.notice?.text).toBe(sentences.notFound)
    expect(state.pending?.kind).toBe("list")
    expect(state.confirming).toBeNull()
  })

  it("U17: busy says so and gives the controls back, listing nothing", () => {
    const state = answer(saving(), no({ kind: "busy" }))
    expect(state.notice?.text).toBe(sentences.busy)
    expect(state.pending).toBeNull()
    expect(canWrite(state)).toBe(true)
    expect(state.form?.command).toBe("/bin/new")
  })

  it("the gateway stopping says nothing changed and gives the controls back, listing nothing", () => {
    const state = answer(saving(), no({ kind: "stopping" }))
    expect(state.notice?.text).toBe("The gateway is stopping, so nothing was changed.")
    expect(state.pending).toBeNull()
    expect(state.form?.command).toBe("/bin/new")
    const removing = answer(
      run(listed(), { type: "askRemove", name: "charts" }, { type: "confirmRemove" }),
      no({ kind: "stopping" }),
    )
    expect(removing.notice?.text).toBe(sentences.stopping)
    expect(removing.pending).toBeNull()
  })

  it.each([
    ["configInvalid", sentences.configInvalid],
    ["configTooLarge", sentences.configTooLarge],
    ["storageUnavailable", sentences.storageUnavailable],
  ] as const)("U18: %s says nothing changed and reloads", (kind, text) => {
    const state = answer(saving(), no({ kind }))
    expect(state.notice?.text).toBe(text)
    expect(state.pending?.kind).toBe("list")
  })

  it("U19: audit unavailable says whether it was applied, with the code, and reloads", () => {
    const applied = answer(
      saving(),
      no({ kind: "auditUnavailable", applied: true, code: "mcp_servers_busy" }),
    )
    expect(applied.notice?.text).toBe(
      "The change was made (mcp_servers_busy), but it couldn't be recorded. The list was reloaded.",
    )
    expect(applied.form).toBeNull()
    expect(applied.pending?.kind).toBe("list")
    const not = answer(saving(), no({ kind: "auditUnavailable", applied: false }))
    expect(not.notice?.text).toBe(
      "Nothing was changed, but it couldn't be recorded. The list was reloaded.",
    )
    expect(not.form?.command).toBe("/bin/new")
    const unknown = answer(saving(), no({ kind: "auditUnavailable" }))
    expect(unknown.notice?.text).toMatch(/^Whether the change happened isn't known/)
  })

  it("the reserved name says so, naming the list's managed server, listing nothing", () => {
    const state = answer(saving(), no({ kind: "reservedName" }))
    expect(state.notice?.text).toBe(
      "“nessa” is Nessa's own server, and can't be changed here.",
    )
    expect(state.pending).toBeNull()
    // The name is the list's, not a copy of the gateway's: another managed
    // server's name is the one said.
    const renamed = answer(
      run(
        listed(listOf(charts, { ...nessa, name: "own" })),
        { type: "edit", name: "charts" },
        { type: "save" },
      ),
      no({ kind: "reservedName" }),
    )
    expect(renamed.notice?.text).toBe(
      "“own” is Nessa's own server, and can't be changed here.",
    )
    expect(sentences.reservedName(undefined)).not.toContain("nessa")
  })

  it("an unanswered write is not confirmed: the list shows where things stand", () => {
    const state = answer(saving(), no({ kind: "unanswered" }))
    expect(state.notice?.text).toBe(sentences.unanswered)
    expect(state.pending?.kind).toBe("list")
    expect(state.form?.command).toBe("/bin/new")
  })

  it("not configured on a write is U4", () => {
    const state = answer(saving(), no({ kind: "notConfigured" }))
    expect(state.list.phase).toBe("notConfigured")
    expect(state.form).toBeNull()
  })
})

describe("in flight", () => {
  it("U21: one request at a time, every write refused while it runs", () => {
    const state = run(listed(), { type: "toggle", name: "charts" })
    expect(canWrite(state)).toBe(false)
    for (const event of [
      { type: "toggle", name: "charts" },
      { type: "add" },
      { type: "edit", name: "charts" },
      { type: "askRemove", name: "charts" },
      { type: "retry" },
    ] as const)
      expect(run(state, event).pending).toBe(state.pending)
    // Inspect rests too: every control, while one request runs.
    expect(canInspect(state)).toBe(false)
    expect(run(state, { type: "inspect", name: "charts" }).inspection).toBeNull()
    const editing = run(listed(), { type: "edit", name: "charts" }, { type: "save" })
    expect(run(editing, { type: "change", patch: { name: "z" } }).form?.name).toBe(
      "charts",
    )
    expect(run(editing, { type: "cancelForm" }).form).not.toBeNull()
    expect(run(editing, { type: "save" }).pending).toBe(editing.pending)
  })
})

describe("inspecting", () => {
  const result: Inspection = {
    complete: true,
    tools: [
      {
        name: "show_chart",
        readOnly: true,
        ui: { uri: "ui://c", csp: [], permissions: [] },
      },
      { name: "app_delete_row", destructive: true },
    ],
  }
  const started = (state = listed(), name = "charts") =>
    run(state, { type: "inspect", name })
  const seqOf = (state: McpServersState) =>
    state.inspection?.phase === "running" ? state.inspection.seq : -1

  it("U22: running, says what it starts, the deadline published, and holds other inspections", () => {
    const state = started()
    expect(state.inspection).toMatchObject({ phase: "running", name: "charts" })
    expect(canInspect(state)).toBe(false)
    // Writes go on: an inspection writes nothing.
    expect(canWrite(state)).toBe(true)
    expect(sentences.inspecting("charts", limits.inspectDeadlineMs)).toBe(
      "Starting “charts”… It has up to 30 seconds.",
    )
    expect(run(state, { type: "closeInspection" }).inspection).toBe(state.inspection)
  })

  it("U23: complete, the tools as answered", () => {
    const state = started()
    const done = run(state, { type: "inspected", seq: seqOf(state), outcome: ok(result) })
    expect(done.inspection).toEqual({ phase: "done", name: "charts", result })
    expect(run(done, { type: "closeInspection" }).inspection).toBeNull()
  })

  it("U24: cut, with the cut's sentence", () => {
    const state = started()
    const cut = { ...result, complete: false, cut: "bytes" as const }
    const done = run(state, { type: "inspected", seq: seqOf(state), outcome: ok(cut) })
    expect(done.inspection).toMatchObject({ phase: "done", result: { cut: "bytes" } })
    expect(sentences.cut.bytes).toBe(
      "The answer was too long; tools were left off the end.",
    )
  })

  it.each([
    [{ kind: "startFailed" }, "“charts” couldn't be started. Check its command."],
    [{ kind: "timedOut" }, "“charts” didn't finish within 30 seconds."],
    [{ kind: "gone" }, "“charts” stopped before it answered."],
    [{ kind: "malformed" }, "“charts” answered with something that isn't MCP."],
    [
      { kind: "remoteError", code: -32601, message: "no such method" },
      "“charts” answered with an error: no such method (-32601)",
    ],
    [{ kind: "remoteError" }, "“charts” answered with an error."],
    [{ kind: "busy" }, "Other servers are being inspected. Try again in a moment."],
    [{ kind: "stopping" }, "The gateway is stopping, so “charts” wasn't started."],
    [
      { kind: "auditUnavailable", applied: true },
      "The server was started, but it couldn't be recorded. The list was reloaded.",
    ],
    [{ kind: "notFound" }, sentences.notFound],
    [{ kind: "unanswered" }, sentences.unanswered],
  ] as [Failure, string][])("U25: %o says its sentence, and Close", (failure, text) => {
    const state = started()
    const failed = run(state, {
      type: "inspected",
      seq: seqOf(state),
      outcome: no(failure),
    })
    expect(failed.inspection).toEqual({ phase: "failed", name: "charts", text })
    expect(run(failed, { type: "closeInspection" }).inspection).toBeNull()
  })

  it("U25: not found lists again", () => {
    const state = started()
    const failed = run(state, {
      type: "inspected",
      seq: seqOf(state),
      outcome: no({ kind: "notFound" }),
    })
    expect(failed.pending?.kind).toBe("list")
  })

  it("U26: a server turned off can be inspected", () => {
    const state = started(listed(listOf({ ...charts, enabled: false }, nessa)))
    expect(state.inspection).toMatchObject({ phase: "running", name: "charts" })
  })

  it("a server removed while it was inspected: the panel says it is gone", () => {
    // Inspected, removed while it ran; the inspection answers, then the list.
    let state = started()
    state = run(state, { type: "askRemove", name: "charts" }, { type: "confirmRemove" })
    state = answer(state, ok(undefined))
    state = run(state, { type: "inspected", seq: seqOf(state), outcome: ok(result) })
    state = answer(state, ok(listOf(nessa)))
    expect(state.inspection).toEqual({
      phase: "failed",
      name: "charts",
      text: "“charts” is no longer stored.",
    })
    // The list first, then the inspection's answer: the same.
    let later = started()
    later = run(later, { type: "askRemove", name: "charts" }, { type: "confirmRemove" })
    later = answer(answer(later, ok(undefined)), ok(listOf(nessa)))
    expect(later.inspection?.phase).toBe("running")
    later = run(later, { type: "inspected", seq: seqOf(later), outcome: ok(result) })
    expect(later.inspection).toEqual(state.inspection)
    // A server still listed keeps what was found.
    const kept = answer(
      run(
        started(),
        { type: "inspected", seq: seqOf(started()), outcome: ok(result) },
        { type: "retry" },
      ),
      ok(listOf(charts, nessa)),
    )
    expect(kept.inspection?.phase).toBe("done")
  })

  it("an answer for another inspection changes nothing", () => {
    const state = started()
    expect(
      run(state, { type: "inspected", seq: seqOf(state) + 1, outcome: ok(result) }),
    ).toBe(state)
  })
})

describe("the connection", () => {
  it("U28: unreachable disables every control, and the next connection lists again", () => {
    let state = run(listed(), { type: "unreachable" })
    expect(state.connection).toBe("unreachable")
    expect(canWrite(state)).toBe(false)
    expect(canInspect(state)).toBe(false)
    expect(run(state, { type: "add" }).form).toBeNull()
    state = run(state, { type: "connected", mayManage: true })
    expect(state.connection).toBe("connected")
    expect(state.pending?.kind).toBe("list")
  })

  it("U28: a request in flight as the connection returns is answered first, not doubled", () => {
    const state = run(
      listed(),
      { type: "toggle", name: "charts" },
      { type: "unreachable" },
      { type: "connected", mayManage: true },
    )
    expect(state.pending?.kind).toBe("save")
  })
})
