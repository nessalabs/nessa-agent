/**
 * Settings › Integrations' reducer, one row of the design's state table
 * (#391 PR 3, U1–U31, U32–U43 from its review, and U44–U49 for a list too
 * large to show) at least one test, each
 * named for its row. Rows the
 * reducer cannot see — the pending row with no gateway (U1), skeletons (U3),
 * widths (U29, U30) and the DOM (U31) — are `integrations-tab.test.tsx`'s and
 * the browser check's (`verification/desktop/scripts/mcp-servers-gateway.mjs`).
 */
import { describe, expect, it } from "vitest"
import {
  canInspect,
  canRemoveByName,
  canWrite,
  editedServer,
  formReady,
  launchChanged,
  valuesNeeded,
  initialMcpServersState,
  mcpServersReducer,
  sentences,
  sharesName,
  groupsOf,
  hasLineBreak,
  lineBreaks,
  linesOf,
  endsWithLineBreak,
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

/** The key of the form's variable row named `name`. */
function keyOf(state: McpServersState, name: string) {
  const row = state.form?.env.find((each) => each.name === name)
  if (!row) throw new Error(`no variable ${name}`)
  return row.key
}

/** Arguments typed into new rows, one each. */
function typedArgs(state: McpServersState, ...values: string[]) {
  return values.reduce((typed, value) => {
    const next = run(typed, { type: "addArgument" })
    const key = next.form!.args[next.form!.args.length - 1].key
    return run(next, { type: "changeArgument", key, value })
  }, state)
}

/** Connected as an administrator, the list answered. */
function listed(list: ServerList = listOf(charts, nessa)) {
  return answer(run(initialMcpServersState(limits), { type: "connected" }), ok(list))
}

describe("access", () => {
  it("U2 (A1): access is unknown until the gateway answers, and a list is asked for", () => {
    const state = run(initialMcpServersState(limits), { type: "connected" })
    expect(state.access).toBe("unknown")
    expect(state.pending).toEqual({ kind: "list", seq: 1 })
    expect(canWrite(state)).toBe(false)
  })

  it("U2 (A2): a list given is the gateway's yes", () => {
    expect(listed().access).toBe("admin")
  })

  it("U2 (A3): a list refused forbidden is the admin notice, nothing offered", () => {
    const state = answer(
      run(initialMcpServersState(limits), { type: "connected" }),
      no({ kind: "forbidden" }),
    )
    expect(state.access).toBe("notAdmin")
    expect(state.pending).toBeNull()
    expect(canWrite(state)).toBe(false)
    expect(canInspect(state)).toBe(false)
    expect(run(state, { type: "retry" }).pending).toBeNull()
    expect(run(state, { type: "add" }).form).toBeNull()
  })

  it("U2 (A4): a connection again asks again, and the answer stands", () => {
    let state = answer(
      run(initialMcpServersState(limits), { type: "connected" }),
      no({ kind: "forbidden" }),
    )
    state = run(state, { type: "unreachable" }, { type: "connected" })
    expect(state.pending?.kind).toBe("list")
    expect(answer(state, no({ kind: "forbidden" })).access).toBe("notAdmin")
    expect(answer(state, ok(listOf(charts))).access).toBe("admin")
  })

  it("U2 (A5): a list that failed otherwise may be tried again", () => {
    const state = answer(
      run(initialMcpServersState(limits), { type: "connected" }),
      no({ kind: "unanswered" }),
    )
    expect(state.access).toBe("unknown")
    expect(run(state, { type: "retry" }).pending?.kind).toBe("list")
  })

  it("U20 (A6): forbidden mid-session is U2, whatever was open", () => {
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
    const state = run(initialMcpServersState(limits), { type: "connected" })
    expect(state.pending).toEqual({ kind: "list", seq: 1 })
    expect(state.list.phase).toBe("loading")
    expect(canWrite(state)).toBe(false)
    expect(run(state, { type: "add" }).form).toBeNull()
  })

  it("U4: not configured says so and offers no Add", () => {
    const state = answer(
      run(initialMcpServersState(limits), { type: "connected" }),
      no({ kind: "notConfigured" }),
    )
    expect(state.list.phase).toBe("notConfigured")
    expect(canWrite(state)).toBe(false)
    expect(run(state, { type: "add" }).form).toBeNull()
  })

  it("U5: only the managed server is an empty list, Add offered", () => {
    const state = listed(listOf(nessa))
    expect(state.list).toMatchObject({ phase: "listed", list: listOf(nessa) })
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
    const state = run(initialMcpServersState(limits), { type: "connected" })
    expect(run(state, { type: "answered", seq: 99, outcome: ok(listOf(charts)) })).toBe(
      state,
    )
  })

  it("a list that succeeds after a reconnect clears the failed list's notice", () => {
    // The list fails, the connection drops and returns, the list succeeds:
    // the failure is answered, and its notice goes.
    let state = answer(
      run(initialMcpServersState(limits), { type: "connected" }),
      no({ kind: "configInvalid" }),
    )
    const unreadable = sentences.listRefused(
      "the configuration file can't be read as it is",
    )
    expect(state.notice).toEqual({ text: unreadable, from: "list" })
    state = run(state, { type: "unreachable" }, { type: "connected" })
    expect(state.notice?.text).toBe(unreadable)
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
    state = run(state, { type: "unreachable" }, { type: "connected" })
    state = answer(state, ok(listOf(charts, nessa)))
    expect(state.notice?.text).toBe(sentences.unanswered)
    expect(run(state, { type: "add" }).notice).toBeNull()
  })

  it("a list that fails says so and lists again only when asked", () => {
    let state = answer(
      run(initialMcpServersState(limits), { type: "connected" }),
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
      args: [],
      env: [],
      enabled: true,
    })
    expect(formReady(state.form!, undefined)).toBe(false)
    expect(run(state, { type: "save" }).pending).toBeNull()
    state = run(state, { type: "change", patch: { name: "x" } })
    expect(formReady(state.form!, undefined)).toBe(false)
    state = run(state, { type: "change", patch: { command: "/bin/x" } })
    expect(formReady(state.form!, undefined)).toBe(true)
  })

  it("U8: a saved server closes the form and lists again", () => {
    let state = typedArgs(
      run(
        listed(),
        { type: "add" },
        { type: "change", patch: { name: "maps", command: "/bin/maps" } },
      ),
      "a",
      "b",
    )
    state = run(state, { type: "addVariable" })
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
      args: [{ value: "server.mjs" }, { value: "--port" }, { value: "1" }],
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

  it("arguments are rows: an empty one and one with a line break round-trip as typed", () => {
    // Listed, edited, saved unchanged: the same arguments.
    const odd = { ...charts, args: ["", "a\nb", "a\r\nb"] }
    const saved = run(
      listed(listOf(odd, nessa)),
      { type: "edit", name: "charts" },
      {
        type: "save",
      },
    )
    expect(saved.pending).toMatchObject({
      request: { server: { args: ["", "a\nb", "a\r\nb"] } },
    })
    // Typed: the same.
    const typed = run(
      typedArgs(
        run(
          listed(),
          { type: "add" },
          {
            type: "change",
            patch: { name: "maps", command: "/bin/maps" },
          },
        ),
        "",
        "a\nb",
      ),
      { type: "save" },
    )
    expect(typed.pending).toMatchObject({ request: { server: { args: ["", "a\nb"] } } })
    // Removed by its own row, the others kept.
    const [first, second] = typed.form!.args
    const removed = run(
      run(listed(), { type: "add" }),
      { type: "addArgument" },
      { type: "addArgument" },
    )
    const [a, b] = removed.form!.args
    expect(run(removed, { type: "removeArgument", key: a.key }).form!.args).toEqual([b])
    expect(first.value).toBe("")
    expect(second.value).toBe("a\nb")
  })
})

describe("a changed launch (U32–U35)", () => {
  const editing = () => run(listed(), { type: "edit", name: "charts" })

  it("U32: the launch unchanged keeps every stored value, Save ready", () => {
    const state = editing()
    expect(launchChanged(state.form!, editedServer(state))).toBe(false)
    expect(valuesNeeded(state.form!, editedServer(state))).toBe(false)
    expect(formReady(state.form!, editedServer(state))).toBe(true)
  })

  it("U33: another command needs every stored value again before Save", () => {
    let state = run(editing(), { type: "change", patch: { command: "/bin/new" } })
    expect(launchChanged(state.form!, editedServer(state))).toBe(true)
    expect(valuesNeeded(state.form!, editedServer(state))).toBe(true)
    expect(formReady(state.form!, editedServer(state))).toBe(false)
    expect(run(state, { type: "save" }).pending).toBeNull()
    state = run(state, {
      type: "changeVariable",
      key: keyOf(state, "TOKEN"),
      patch: { value: "again" },
    })
    expect(formReady(state.form!, editedServer(state))).toBe(true)
    expect(run(state, { type: "save" }).pending).toMatchObject({
      request: {
        server: { command: "/bin/new", env: [{ name: "TOKEN", value: "again" }] },
      },
    })
  })

  it("U33: other arguments need every stored value again too", () => {
    const state = editing()
    const removed = run(state, { type: "removeArgument", key: state.form!.args[2].key })
    expect(launchChanged(removed.form!, editedServer(removed))).toBe(true)
    expect(formReady(removed.form!, editedServer(removed))).toBe(false)
    const changed = run(state, {
      type: "changeArgument",
      key: state.form!.args[0].key,
      value: "other.mjs",
    })
    expect(formReady(changed.form!, editedServer(changed))).toBe(false)
    const added = typedArgs(state, "--more")
    expect(formReady(added.form!, editedServer(added))).toBe(false)
    // A stored variable removed needs no value; a renamed server alone is no new launch.
    const without = run(removed, { type: "removeVariable", key: keyOf(removed, "TOKEN") })
    expect(formReady(without.form!, editedServer(without))).toBe(true)
    const renamed = run(state, { type: "change", patch: { name: "graphs" } })
    expect(formReady(renamed.form!, editedServer(renamed))).toBe(true)
  })

  it("U34: changed back to the listed launch, the stored values are kept again", () => {
    let state = run(editing(), { type: "change", patch: { command: "/bin/new" } })
    state = run(state, { type: "change", patch: { command: charts.command } })
    expect(launchChanged(state.form!, editedServer(state))).toBe(false)
    expect(formReady(state.form!, editedServer(state))).toBe(true)
    const args = editing()
    const key = args.form!.args[0].key
    const back = run(
      args,
      { type: "changeArgument", key, value: "x" },
      { type: "changeArgument", key, value: "server.mjs" },
    )
    expect(launchChanged(back.form!, editedServer(back))).toBe(false)
    expect(run(back, { type: "save" }).pending).toMatchObject({
      request: { server: { env: [{ name: "TOKEN", value: null }] } },
    })
  })

  it("U35: a value refused as missing after a changed launch says why", () => {
    // The list changed under the form: the gateway is the judge.
    let state = run(editing(), { type: "change", patch: { command: "/bin/new" } })
    state = run(state, {
      type: "changeVariable",
      key: keyOf(state, "TOKEN"),
      patch: { value: "again" },
    })
    state = answer(
      run(state, { type: "save" }),
      no({ kind: "invalid", problem: "environmentValueMissing", name: "OTHER" }),
    )
    expect(state.form?.problem).toEqual({ field: "env", text: sentences.valuesAgain })
    expect(sentences.valuesAgain).toBe(
      "Changing the command or arguments needs every value entered again.",
    )
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
    expect(state.confirming).toEqual({ name: "charts", count: 1 })
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
  /** An edit changing the command, so its stored value is entered again (U33). */
  const saving = () => {
    const state = run(
      listed(),
      { type: "edit", name: "charts" },
      { type: "change", patch: { command: "/bin/new" } },
    )
    const sent = run(
      state,
      { type: "changeVariable", key: keyOf(state, "TOKEN"), patch: { value: "again" } },
      { type: "save" },
    )
    expect(sent.pending).toMatchObject({
      request: { server: { env: [{ name: "TOKEN", value: "again" }] } },
    })
    return sent
  }

  it("U15: a conflict reloads, says so, and keeps what was typed", () => {
    const state = answer(saving(), no({ kind: "revisionConflict" }))
    expect(state.notice?.text).toBe(sentences.conflict)
    expect(state.form?.command).toBe("/bin/new")
    expect(state.pending?.kind).toBe("list")
  })

  it("U41: the server edited no longer listed after the reload closes the form, saying so", () => {
    const state = answer(
      answer(saving(), no({ kind: "revisionConflict" })),
      ok(listOf(nessa)),
    )
    expect(state.form).toBeNull()
    expect(state.notice?.text).toBe(
      "“charts” is no longer stored, so the form was closed.",
    )
  })

  it("U42: the reload refills what was not typed, and names what was typed and changed there too", () => {
    const conflicted = answer(saving(), no({ kind: "revisionConflict" }))
    const elsewhere: ListedServer = {
      ...charts,
      command: "/bin/theirs",
      args: ["theirs.mjs"],
      envNames: ["TOKEN", "REGION"],
      enabled: false,
    }
    const state = answer(conflicted, ok({ revision: "r2", servers: [elsewhere, nessa] }))
    // Typed here: kept, and named.
    expect(state.form?.command).toBe("/bin/new")
    // Untouched: the other window's.
    expect(state.form?.args.map((row) => row.value)).toEqual(["theirs.mjs"])
    expect(state.form?.enabled).toBe(false)
    expect(state.form?.env.map((row) => [row.name, row.value, row.stored])).toEqual([
      ["TOKEN", "again", true],
      ["REGION", "", true],
    ])
    expect(state.notice?.text).toBe(
      `${sentences.conflict} Changed elsewhere too, and kept as typed here: the command.`,
    )
    // The next save is judged against the list now.
    expect(state.form?.base).toEqual(elsewhere)
    expect(launchChanged(state.form!, editedServer(state))).toBe(true)
    expect(formReady(state.form!, editedServer(state))).toBe(false)
  })

  it("U42: an untouched command and arguments follow the reload; a touched switch is kept", () => {
    const editing = run(
      listed(),
      { type: "edit", name: "charts" },
      { type: "change", patch: { enabled: false } },
    )
    const elsewhere = { ...charts, command: "/bin/theirs", args: ["x"] }
    const state = answer(run(editing, { type: "retry" }), ok(listOf(elsewhere, nessa)))
    expect(state.form?.command).toBe("/bin/theirs")
    expect(state.form?.args.map((row) => row.value)).toEqual(["x"])
    expect(state.form?.enabled).toBe(false)
    expect(state.notice).toBeNull()
    // Refilled to the list, the launch is the listed one: the value is kept.
    expect(launchChanged(state.form!, editedServer(state))).toBe(false)
  })

  it("U42: nothing changed there leaves the form as typed, the notice alone", () => {
    const conflicted = answer(saving(), no({ kind: "revisionConflict" }))
    const state = answer(conflicted, ok(listOf(charts, nessa)))
    expect(state.form).toEqual(conflicted.form)
    expect(state.notice?.text).toBe(sentences.conflict)
  })

  it("U42: a variable removed there goes if untouched, and stays as a new one if typed", () => {
    const untouched = run(listed(), { type: "edit", name: "charts" })
    const gone = answer(
      run(untouched, { type: "retry" }),
      ok(listOf({ ...charts, envNames: [] }, nessa)),
    )
    expect(gone.form?.env).toEqual([])
    expect(gone.notice).toBeNull()
    const typed = run(untouched, {
      type: "changeVariable",
      key: keyOf(untouched, "TOKEN"),
      patch: { value: "mine" },
    })
    const kept = answer(
      run(typed, { type: "retry" }),
      ok(listOf({ ...charts, envNames: [] }, nessa)),
    )
    expect(kept.form?.env).toMatchObject([
      { name: "TOKEN", value: "mine", stored: false },
    ])
    expect(kept.notice?.text).toBe(
      "Changed elsewhere too, and kept as typed here: the variables.",
    )
  })

  it("a variable problem with no name, or an empty one, is said of “A variable”", () => {
    for (const name of [undefined, ""]) {
      const state = answer(
        run(listed(), { type: "edit", name: "charts" }, { type: "save" }),
        no({
          kind: "invalid",
          problem: "environmentName",
          ...(name === undefined ? {} : { name }),
        }),
      )
      expect(state.form?.problem?.text).toBe(
        "A variable isn't a name a variable can have.",
      )
    }
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
    [{ kind: "configInvalid" }, sentences.configInvalid],
    [{ kind: "storageUnavailable", applied: false }, sentences.storageUnavailable],
  ] as [Failure, string][])(
    "U18, U37: %o says nothing changed, keeps the form and reloads",
    (failure, text) => {
      const state = answer(saving(), no(failure))
      expect(state.notice?.text).toBe(text)
      expect(state.form?.command).toBe("/bin/new")
      expect(state.pending?.kind).toBe("list")
    },
  )

  it("U36: storage unavailable but applied is saved, may not survive a crash, and closes the form", () => {
    const state = answer(saving(), no({ kind: "storageUnavailable", applied: true }))
    expect(state.notice?.text).toBe("Saved, but it may not survive a crash.")
    expect(state.form).toBeNull()
    expect(state.pending?.kind).toBe("list")
    const removed = answer(
      run(listed(), { type: "askRemove", name: "charts" }, { type: "confirmRemove" }),
      no({ kind: "storageUnavailable", applied: true }),
    )
    expect(removed.notice?.text).toBe("Removed, but it may not survive a crash.")
    expect(removed.confirming).toBeNull()
    expect(removed.pending?.kind).toBe("list")
  })

  it("U38: storage unavailable without its details claims neither, keeps the form and reloads", () => {
    const state = answer(saving(), no({ kind: "storageUnavailable" }))
    expect(state.notice?.text).toBe(sentences.storageUnknown)
    expect(state.notice?.text).not.toMatch(/nothing was changed/)
    expect(state.form?.command).toBe("/bin/new")
    expect(state.pending?.kind).toBe("list")
  })

  it("U19, U43: audit unavailable says whether it was applied, what stopped it in words, and reloads", () => {
    const applied = answer(
      saving(),
      no({ kind: "auditUnavailable", applied: true, cause: "busy" }),
    )
    expect(applied.notice?.text).toBe(
      "The change was made, but it couldn't be recorded (another change was in progress). The list was reloaded.",
    )
    expect(applied.notice?.text).not.toMatch(/mcp_|_/)
    const durable = answer(
      saving(),
      no({ kind: "auditUnavailable", applied: true, cause: "storageUnavailable" }),
    )
    expect(durable.notice?.text).toBe(
      "The change was made, but it couldn't be recorded (it may not survive a crash). The list was reloaded.",
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

  it("U24: a stopping cut, with no tools, is a done inspection that says so", () => {
    const state = started()
    const stopping = { complete: false, cut: "stopping" as const, tools: [] }
    const done = run(state, {
      type: "inspected",
      seq: seqOf(state),
      outcome: ok(stopping),
    })
    expect(done.inspection).toEqual({ phase: "done", name: "charts", result: stopping })
    expect(sentences.cut.stopping).toBe(
      "The gateway began to stop, so the server was stopped before its tools were read.",
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
      "“charts” may have started, but the inspection couldn't be recorded.",
    ],
    [
      { kind: "auditUnavailable", applied: false, cause: "startFailed" },
      "“charts” wasn't started, and the inspection couldn't be recorded (the server couldn't be started).",
    ],
    [
      { kind: "auditUnavailable" },
      "Whether “charts” started isn't known, and the inspection couldn't be recorded.",
    ],
    [{ kind: "notFound" }, sentences.notFound],
    [
      { kind: "unanswered" },
      "“charts”'s inspection didn't answer in time, or the connection was lost.",
    ],
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
    state = run(state, { type: "connected" })
    expect(state.connection).toBe("connected")
    expect(state.pending?.kind).toBe("list")
  })

  it("U28: a request in flight as the connection returns is answered first, not doubled", () => {
    const state = run(
      listed(),
      { type: "toggle", name: "charts" },
      { type: "unreachable" },
      { type: "connected" },
    )
    expect(state.pending?.kind).toBe("save")
  })
})

describe("a list too large to show (U44–U49)", () => {
  const tooLarge = (revision?: string): Failure =>
    revision === undefined
      ? { kind: "configTooLarge" }
      : { kind: "configTooLarge", revision }

  /** Connected, the list refused as too large at `revision`. */
  function refused(revision = "r7") {
    return answer(
      run(initialMcpServersState(limits), { type: "connected" }),
      no(tooLarge(revision)),
    )
  }

  it("U44: a list refused with its revision shows no server, keeps the revision, and offers removal by name", () => {
    const state = refused()
    expect(state.list).toEqual({ phase: "tooLarge", revision: "r7", name: "" })
    expect(state.notice).toBeNull()
    expect(canWrite(state)).toBe(false)
    expect(canInspect(state)).toBe(false)
    expect(run(state, { type: "add" }).form).toBeNull()
    expect(canRemoveByName(state)).toBe(false)
    expect(canRemoveByName(run(state, { type: "changeRemoveName", name: "big" }))).toBe(
      true,
    )
    expect(sentences.listTooLarge).toBe(
      "The server list is too large to show. Removing a server fixes it: enter its name.",
    )
  })

  it("U44: a list read after a write that comes back too large closes the form", () => {
    // A switch saved, then the form opened while the list is read again.
    let state = answer(run(listed(), { type: "toggle", name: "charts" }), ok(undefined))
    state = { ...state, form: run(listed(), { type: "edit", name: "charts" }).form }
    expect(state.form?.editing).toBe("charts")
    state = answer(state, no(tooLarge("r8")))
    expect(state.list).toEqual({ phase: "tooLarge", revision: "r8", name: "" })
    expect(state.form).toBeNull()
    expect(state.confirming).toBeNull()
  })

  it("U45: confirmed, the name typed is removed at the refusal's revision, and the list read again", () => {
    let state = run(
      refused(),
      { type: "changeRemoveName", name: "big" },
      { type: "askRemoveByName" },
    )
    expect(state.confirming).toEqual({ name: "big", count: null })
    expect(state.pending).toBeNull()
    state = run(state, { type: "confirmRemove" })
    expect(state.pending).toMatchObject({
      kind: "remove",
      request: { revision: "r7", name: "big" },
    })
    state = answer(state, ok(undefined))
    expect(state.confirming).toBeNull()
    expect(state.pending?.kind).toBe("list")
    expect(state.list.phase === "tooLarge" && state.list.name).toBe("")
    const fits = answer(state, ok(listOf(charts, nessa)))
    expect(fits.list.phase).toBe("listed")
    const still = answer(state, no(tooLarge("r9")))
    expect(still.list).toEqual({ phase: "tooLarge", revision: "r9", name: "" })
  })

  it("U45: nothing is asked or sent without a name, or while a request is in flight", () => {
    const empty = run(refused(), { type: "askRemoveByName" }, { type: "confirmRemove" })
    expect(empty.confirming).toBeNull()
    expect(empty.pending).toBeNull()
    const typed = run(refused(), { type: "changeRemoveName", name: "big" })
    const away = run(typed, { type: "unreachable" }, { type: "askRemoveByName" })
    expect(away.confirming).toBeNull()
    // Retyping the name withdraws the question about the old one.
    const retyped = run(
      typed,
      { type: "askRemoveByName" },
      { type: "changeRemoveName", name: "other" },
    )
    expect(retyped.confirming).toBeNull()
  })

  it("U46: a name not stored says so, keeps the name typed, and lists again", () => {
    let state = run(
      refused(),
      { type: "changeRemoveName", name: "bigg" },
      { type: "askRemoveByName" },
      { type: "confirmRemove" },
    )
    state = answer(state, no({ kind: "notFound" }))
    expect(state.notice?.text).toBe("No server is stored under “bigg”.")
    expect(state.confirming).toBeNull()
    expect(state.pending?.kind).toBe("list")
    state = answer(state, no(tooLarge("r7")))
    expect(state.list).toEqual({ phase: "tooLarge", revision: "r7", name: "bigg" })
    // The write's notice stands over the list read after it.
    expect(state.notice?.text).toBe("No server is stored under “bigg”.")
  })

  it("U46: a row's removal of a server gone keeps its own sentence", () => {
    let state = run(
      listed(),
      { type: "askRemove", name: "charts" },
      { type: "confirmRemove" },
    )
    state = answer(state, no({ kind: "notFound" }))
    expect(state.notice?.text).toBe(sentences.notFound)
    expect(state.pending?.kind).toBe("list")
  })

  it("U47: a list refused without a revision fails, says the file is too large, and offers no removal", () => {
    const failed = answer(
      run(initialMcpServersState(limits), { type: "connected" }),
      no(tooLarge()),
    )
    expect(failed.list).toEqual({ phase: "failed" })
    expect(failed.notice?.text).toBe("The configuration file is too large to read here.")
    expect(canRemoveByName(failed)).toBe(false)
    expect(run(failed, { type: "retry" }).pending?.kind).toBe("list")
  })

  it("U48: a form's save refused as too large keeps the form, says why in it, and lists nothing again", () => {
    let state = run(listed(), { type: "edit", name: "charts" })
    state = typedArgs(state, "x".repeat(100))
    state = run(state, {
      type: "changeVariable",
      key: keyOf(state, "TOKEN"),
      patch: { value: "v" },
    })
    state = run(state, { type: "save" })
    expect(state.pending?.kind).toBe("save")
    state = answer(state, no(tooLarge()))
    expect(state.pending).toBeNull()
    expect(state.form?.editing).toBe("charts")
    expect(state.form?.args.at(-1)?.value).toBe("x".repeat(100))
    expect(state.form?.problem).toEqual({
      field: "form",
      text: "This would make the server list too large; remove a server or shorten its arguments.",
    })
    expect(state.notice).toBeNull()
    expect(state.list.phase).toBe("listed")
  })

  it("U49: a switch refused as too large says so and lists nothing again", () => {
    let state = run(listed(), { type: "toggle", name: "charts" })
    state = answer(state, no(tooLarge()))
    expect(state.pending).toBeNull()
    expect(state.notice?.text).toBe(sentences.saveTooLarge)
    expect(canWrite(state)).toBe(true)
  })
})

describe("stored servers sharing a name (G1–G9)", () => {
  const first: ListedServer = { ...charts, args: ["first.mjs"] }
  const second: ListedServer = { ...charts, args: ["second.mjs"], enabled: false }
  const third: ListedServer = { ...charts, args: ["third.mjs"] }
  const other: ListedServer = { ...charts, name: "docs" }
  const thrice = () => listed(listOf(other, first, second, third, nessa))
  const idsOf = (state: McpServersState) =>
    state.list.phase === "listed" ? state.list.ids : []

  it("G1: each listed server has an occurrence id, kept by the k-th of its name", () => {
    const state = thrice()
    const ids = idsOf(state)
    expect(new Set(ids).size).toBe(5)
    // The first "charts" goes, the docs row stays: docs keeps its id, the
    // remaining two "charts" take the first two ids "charts" had.
    const after = run(state, { type: "unreachable" }, { type: "connected" })
    const relisted = answer(after, ok(listOf(other, second, third, nessa)))
    expect(idsOf(relisted)).toEqual([ids[0], ids[1], ids[2], ids[4]])
    // A server new to the list has an id no other row had.
    const grown = answer(
      run(relisted, { type: "unreachable" }, { type: "connected" }),
      ok(listOf({ ...charts, name: "new" }, other, second, third, nessa)),
    )
    expect(idsOf(grown).slice(1)).toEqual(idsOf(relisted))
    expect(ids).not.toContain(idsOf(grown)[0])
    expect(idsOf(relisted)).not.toContain(idsOf(grown)[0])
  })

  it("G1: the ids are not the rows' places", () => {
    const state = thrice()
    const removed = answer(
      run(state, { type: "unreachable" }, { type: "connected" }),
      ok(listOf(first, second, third, nessa)),
    )
    // docs, the first, went: "charts" rows keep theirs.
    expect(idsOf(removed)).toEqual(idsOf(state).slice(1))
  })

  it("G2: a name stored once is not shared; the managed one never is", () => {
    const state = thrice()
    expect(sharesName(state, "docs")).toBe(false)
    expect(sharesName(state, "nessa")).toBe(false)
    expect(sharesName(listed(listOf(charts, nessa)), "charts")).toBe(false)
  })

  it("G3: a name stored three times is one group of three, at the first one's place", () => {
    const state = thrice()
    if (state.list.phase !== "listed") throw new Error("not listed")
    const { groups, managed } = groupsOf(state.list)
    expect(groups.map((group) => [group.name, group.rows.length])).toEqual([
      ["docs", 1],
      ["charts", 3],
    ])
    expect(groups[1].rows.map((row) => row.server)).toEqual([first, second, third])
    expect(managed.map((row) => row.server)).toEqual([nessa])
    expect(sentences.nameShared(3)).toBe(
      "3 servers share this name. Only the first can be removed here, and none edited.",
    )
  })

  it("G4: a shared name is not edited, switched or inspected", () => {
    const state = thrice()
    expect(run(state, { type: "edit", name: "charts" })).toBe(state)
    expect(run(state, { type: "toggle", name: "charts" })).toBe(state)
    expect(run(state, { type: "inspect", name: "charts" })).toBe(state)
    // The name stored once still is.
    expect(run(state, { type: "edit", name: "docs" }).form?.editing).toBe("docs")
    expect(run(state, { type: "toggle", name: "docs" }).pending?.kind).toBe("save")
    expect(run(state, { type: "inspect", name: "docs" }).inspection?.phase).toBe(
      "running",
    )
  })

  it("G5: the group's action asks to remove the first, with how many share it", () => {
    const state = run(thrice(), { type: "askRemove", name: "charts" })
    expect(state.confirming).toEqual({ name: "charts", count: 3 })
    expect(sentences.removeFirst("charts")).toBe("Remove the first server named “charts”")
    expect(sentences.removeFirstAsk("charts")).toBe(
      "Remove the first server named “charts”? New conversations stop getting it. Open ones keep it until they close.",
    )
    expect(run(state, { type: "cancelRemove" }).confirming).toBeNull()
  })

  it("G5: a name not stored is not asked about", () => {
    const state = thrice()
    expect(run(state, { type: "askRemove", name: "gone" })).toBe(state)
    expect(run(state, { type: "askRemove", name: "nessa" })).toBe(state)
  })

  it("G6: confirmed, the removal names the server; answered, the list is read", () => {
    let state = run(
      thrice(),
      { type: "askRemove", name: "charts" },
      { type: "confirmRemove" },
    )
    expect(state.pending).toMatchObject({
      kind: "remove",
      request: { revision: "r1", name: "charts" },
    })
    state = answer(state, ok(undefined))
    expect(state.confirming).toBeNull()
    state = answer(state, ok(listOf(other, second, third, nessa)))
    expect(sharesName(state, "charts")).toBe(true)
  })

  it("G7: a list read meanwhile keeps the confirm only while the count holds", () => {
    const asked = run(thrice(), { type: "askRemove", name: "charts" })
    const relisted = run(asked, { type: "unreachable" }, { type: "connected" })
    expect(answer(relisted, ok(listOf(first, second, third, nessa))).confirming).toEqual({
      name: "charts",
      count: 3,
    })
    expect(
      answer(relisted, ok(listOf(other, second, third, nessa))).confirming,
    ).toBeNull()
    expect(answer(relisted, ok(listOf(other, nessa))).confirming).toBeNull()
    // A name stored once, asked as such, is not kept once it is shared.
    const once = run(listed(listOf(charts, nessa)), { type: "askRemove", name: "charts" })
    expect(once.confirming).toEqual({ name: "charts", count: 1 })
    expect(
      answer(
        run(once, { type: "unreachable" }, { type: "connected" }),
        ok(listOf(first, second, nessa)),
      ).confirming,
    ).toBeNull()
  })

  it("G8: a form whose name becomes shared is closed, saying so", () => {
    let state = run(listed(listOf(charts, nessa)), { type: "edit", name: "charts" })
    state = run(state, { type: "unreachable" }, { type: "connected" })
    state = answer(state, ok(listOf(charts, second, nessa)))
    expect(state.form).toBeNull()
    expect(state.notice).toEqual({
      text: "“charts” is now stored more than once, so the form was closed.",
      from: "write",
    })
  })

  it("G9: removing by a typed name says it removes the first stored under it", () => {
    expect(sentences.removeByNameAsk("big")).toBe(
      "Remove “big”? This removes the first server stored under that name. New conversations stop getting it. Open ones keep it until they close.",
    )
  })
})

describe("a variable's value (S1–S8)", () => {
  const edit = () => run(listed(), { type: "edit", name: "charts" })
  const saved = (state: McpServersState) => {
    const next = run(state, { type: "save" })
    if (next.pending?.kind !== "save") throw new Error("no save")
    return next.pending.request.server.env
  }

  it("S1: a stored variable not edited is kept", () => {
    expect(saved(edit())).toEqual([{ name: "TOKEN", value: null }])
  })

  it("S2 (Codex P1): edited and emptied, it saves the empty value; kept again, null", () => {
    let state = edit()
    const key = keyOf(state, "TOKEN")
    state = run(
      state,
      { type: "changeVariable", key, patch: { value: "x" } },
      { type: "changeVariable", key, patch: { value: "" } },
    )
    expect(saved(state)).toEqual([{ name: "TOKEN", value: "" }])
    state = run(state, { type: "keepVariable", key })
    expect(state.form?.env[0]).toMatchObject({ value: "", edited: false, held: false })
    expect(saved(state)).toEqual([{ name: "TOKEN", value: null }])
  })

  it("S2: keeping is for stored variables only", () => {
    let state = run(listed(), { type: "add" }, { type: "addVariable" })
    const key = state.form!.env[0].key
    state = run(state, { type: "changeVariable", key, patch: { value: "v" } })
    expect(run(state, { type: "keepVariable", key }).form).toEqual(state.form)
  })

  it("S3: typed, the value is drawn, not held", () => {
    let state = edit()
    const key = keyOf(state, "TOKEN")
    state = run(state, { type: "changeVariable", key, patch: { value: "abc" } })
    expect(state.form?.env[0]).toMatchObject({ value: "abc", edited: true, held: false })
  })

  it("S4: a paste with line breaks is held raw, CRLF and all, and saved as pasted", () => {
    const pem = "-----BEGIN-----\r\nAAA\nBBB\r-----END-----"
    let state = edit()
    const key = keyOf(state, "TOKEN")
    state = run(state, { type: "pasteVariable", key, value: pem })
    expect(state.form?.env[0]).toMatchObject({ value: pem, edited: true, held: true })
    expect(saved(state)).toEqual([{ name: "TOKEN", value: pem }])
    expect(linesOf(pem)).toBe(4)
    expect(sentences.pasted(4)).toBe("Pasted value: 4 lines")
    expect(sentences.pasted(1)).toBe("Pasted value: 1 line")
  })

  it("S4: lines count the breaks but one at the end", () => {
    expect(hasLineBreak("a")).toBe(false)
    expect(hasLineBreak("a\r")).toBe(true)
    expect(lineBreaks("a\r\nb\nc\rd")).toBe(3)
    expect(linesOf("a\nb\n")).toBe(2)
    expect(linesOf("a\n\n")).toBe(2)
    expect(linesOf("\n")).toBe(1)
    expect(linesOf("a\r\n")).toBe(1)
  })

  it("S5: one trailing line break is pointed out and trimmed only when asked", () => {
    let state = edit()
    const key = keyOf(state, "TOKEN")
    state = run(state, { type: "pasteVariable", key, value: "sk-1\n" })
    expect(endsWithLineBreak(state.form!.env[0].value)).toBe(true)
    expect(saved(state)).toEqual([{ name: "TOKEN", value: "sk-1\n" }])
    state = run(state, { type: "trimVariable", key })
    expect(state.form?.env[0]).toMatchObject({ value: "sk-1", held: true, edited: true })
    // Nothing more to trim.
    expect(run(state, { type: "trimVariable", key }).form).toEqual(state.form)
    // A CRLF is one break.
    state = run(state, { type: "pasteVariable", key, value: "a\nb\r\n" })
    expect(run(state, { type: "trimVariable", key }).form?.env[0].value).toBe("a\nb")
    // Two at the end: one goes, and the other is pointed out in turn.
    state = run(state, { type: "pasteVariable", key, value: "a\n\n" })
    expect(run(state, { type: "trimVariable", key }).form?.env[0].value).toBe("a\n")
  })

  it("S5: a value typed is never trimmed", () => {
    let state = edit()
    const key = keyOf(state, "TOKEN")
    state = run(state, { type: "changeVariable", key, patch: { value: "a\n" } })
    expect(run(state, { type: "trimVariable", key }).form).toEqual(state.form)
  })

  it("S6: Clear empties a held value, edited", () => {
    let state = edit()
    const key = keyOf(state, "TOKEN")
    state = run(
      state,
      { type: "pasteVariable", key, value: "a\nb" },
      { type: "clearVariable", key },
    )
    expect(state.form?.env[0]).toMatchObject({ value: "", edited: true, held: false })
    expect(saved(state)).toEqual([{ name: "TOKEN", value: "" }])
  })

  it("S7: with the launch changed, a stored variable waits until edited, empty counts", () => {
    let state = edit()
    const key = keyOf(state, "TOKEN")
    state = run(state, { type: "change", patch: { command: "/usr/bin/other" } })
    expect(valuesNeeded(state.form!, editedServer(state))).toBe(true)
    expect(run(state, { type: "save" }).pending).toBeNull()
    state = run(
      state,
      { type: "changeVariable", key, patch: { value: "x" } },
      { type: "changeVariable", key, patch: { value: "" } },
    )
    expect(valuesNeeded(state.form!, editedServer(state))).toBe(false)
    expect(saved(state)).toEqual([{ name: "TOKEN", value: "" }])
  })

  it("S8: a stored variable removed elsewhere goes if not edited, stays as new if edited", () => {
    const relist = (state: McpServersState) =>
      answer(
        run(state, { type: "unreachable" }, { type: "connected" }),
        ok(listOf({ ...charts, envNames: [] }, nessa)),
      )
    const untouched = relist(edit())
    expect(untouched.form?.env).toEqual([])
    expect(untouched.notice).toBeNull()
    let state = edit()
    const key = keyOf(state, "TOKEN")
    state = run(
      state,
      { type: "changeVariable", key, patch: { value: "x" } },
      { type: "changeVariable", key, patch: { value: "" } },
    )
    const edited = relist(state)
    expect(edited.form?.env).toMatchObject([
      { name: "TOKEN", value: "", stored: false, edited: true },
    ])
    expect(edited.notice?.text).toBe(sentences.changedThere(["the variables"]))
  })
})

describe("also (F8–F12)", () => {
  it("F8: a variable added both here and elsewhere is named in the clashes", () => {
    let state = run(listed(), { type: "edit", name: "charts" }, { type: "addVariable" })
    const key = state.form!.env[1].key
    state = run(state, {
      type: "changeVariable",
      key,
      patch: { name: "OTHER", value: "v" },
    })
    state = answer(
      run(state, { type: "unreachable" }, { type: "connected" }),
      ok(listOf({ ...charts, envNames: ["TOKEN", "OTHER"] }, nessa)),
    )
    expect(state.notice?.text).toBe(sentences.changedThere(["the variables"]))
    // Kept as typed, once: not a second, stored row.
    expect(state.form?.env.filter((row) => row.name === "OTHER")).toMatchObject([
      { stored: false, value: "v" },
    ])
  })

  it("F8: a variable added elsewhere only is added untouched, no clash", () => {
    let state = run(listed(), { type: "edit", name: "charts" })
    state = answer(
      run(state, { type: "unreachable" }, { type: "connected" }),
      ok(listOf({ ...charts, envNames: ["TOKEN", "OTHER"] }, nessa)),
    )
    expect(state.notice).toBeNull()
    expect(state.form?.env.map((row) => [row.name, row.stored])).toEqual([
      ["TOKEN", true],
      ["OTHER", true],
    ])
  })

  it.each([
    ["unanswered", { kind: "unanswered" }],
    ["audit, applied unknown", { kind: "auditUnavailable" }],
    ["storage, applied unknown", { kind: "storageUnavailable" }],
  ] as const)(
    "F9: a rename not confirmed (%s) keeps its notice when the old name is gone",
    (_, failure) => {
      let state = run(
        listed(),
        { type: "edit", name: "charts" },
        { type: "change", patch: { name: "graphs" } },
        { type: "save" },
      )
      state = answer(state, no(failure as Failure))
      const said = state.notice
      expect(said).not.toBeNull()
      state = answer(state, ok(listOf({ ...charts, name: "graphs" }, nessa)))
      expect(state.form).toBeNull()
      expect(state.notice).toEqual(said)
    },
  )

  it("F9: only the list read right after it: a later one says the form is gone", () => {
    let state = run(
      listed(),
      { type: "edit", name: "charts" },
      { type: "change", patch: { name: "graphs" } },
      { type: "save" },
    )
    state = answer(state, no({ kind: "unanswered" }))
    // Not applied: still under its name, and the form is kept.
    state = answer(state, ok(listOf(charts, nessa)))
    expect(state.form?.unconfirmed).toBeUndefined()
    state = answer(
      run(state, { type: "unreachable" }, { type: "connected" }),
      ok(listOf(nessa)),
    )
    expect(state.notice?.text).toBe(sentences.formGone("charts"))
  })

  it("F9: a save refused as applied false is not unconfirmed", () => {
    let state = run(
      listed(),
      { type: "edit", name: "charts" },
      { type: "change", patch: { name: "graphs" } },
      { type: "save" },
    )
    state = answer(state, no({ kind: "storageUnavailable", applied: false }))
    expect(state.form?.unconfirmed).toBeUndefined()
  })

  it.each([
    [{ kind: "unanswered" }, sentences.listFailed],
    [{ kind: "remoteError", code: 1 }, sentences.listFailed],
    [{ kind: "auditUnavailable" }, sentences.listFailed],
    [
      { kind: "storageUnavailable" },
      "The servers couldn't be listed: the configuration file couldn't be read or written.",
    ],
    [
      { kind: "configInvalid" },
      "The servers couldn't be listed: the configuration file can't be read as it is.",
    ],
    [{ kind: "busy" }, "The servers couldn't be listed: another change was in progress."],
    [{ kind: "configTooLarge" }, sentences.configFileTooLarge],
  ] as const)("F10: a list failing %o says it failed", (failure, text) => {
    const state = answer(
      run(initialMcpServersState(limits), { type: "connected" }),
      no(failure as Failure),
    )
    expect(state.list.phase).toBe("failed")
    expect(state.notice).toEqual({ text, from: "list" })
  })

  it("F11: an argument added after a row goes right after it", () => {
    let state = typedArgs(run(listed(), { type: "add" }), "a", "c")
    const firstKey = state.form!.args[0].key
    state = run(state, { type: "addArgument", after: firstKey })
    expect(state.form!.args.map((row) => row.value)).toEqual(["a", "", "c"])
    expect(state.form!.args[1].key).toBe(state.rows)
    // After an unknown row, or none, it goes last.
    state = run(state, { type: "addArgument", after: -5 })
    expect(state.form!.args.map((row) => row.value)).toEqual(["a", "", "c", ""])
  })

  it("F11: an argument with a line break is one argument, its breaks counted", () => {
    const state = typedArgs(run(listed(), { type: "add" }), "a\nb\r\nc")
    expect(state.form!.args.map((row) => row.value)).toEqual(["a\nb\r\nc"])
    expect(lineBreaks(state.form!.args[0].value)).toBe(2)
    expect(sentences.argumentBreaks(1)).toBe("Has a line break")
    expect(sentences.argumentBreaks(2)).toBe("Has 2 line breaks")
  })
})
