/**
 * Settings › Connections › Integrations: the gateway's stored MCP servers, as
 * the window manages them (#391). One reducer, from the design's state table
 * (rows U1–U31, issue #391's PR 3 design); its tests are one row at least one
 * test, in `mcp-servers.test.ts`.
 *
 * The window never retypes a gateway rule (gate 13). Whether a name, command,
 * argument or variable is acceptable is the gateway's to answer, as a typed
 * refusal this reads into its own words here; the only limits it shows are
 * the ones the protocol publishes, handed in (`McpServersLimits`). Nothing is
 * updated optimistically: every write that may have changed the list lists
 * again, and the list is what is shown (gate 16).
 *
 * The state names the one request in flight (`pending`), and the tab sends
 * it; its answer comes back as `answered` with the request's `seq`, so an
 * answer for a request since replaced changes nothing. An inspection is its
 * own slot (`inspection`), since it runs for seconds and writes nothing.
 */

/** A stored server as the gateway lists it. Variable values are never listed; only names. */
export interface ListedServer {
  readonly name: string
  readonly command: string
  readonly args: readonly string[]
  readonly envNames: readonly string[]
  readonly enabled: boolean
  /** Nessa's own server: listed, never edited here. */
  readonly managed: boolean
}

export interface ServerList {
  /** What a write must name; a write at any other is refused as a conflict. */
  readonly revision: string
  /** Stored order, then the managed one. */
  readonly servers: readonly ListedServer[]
}

/** A variable as saved: its value, or `null` to keep the one stored. */
export interface SavedVariable {
  readonly name: string
  readonly value: string | null
}

export interface SaveRequest {
  readonly revision: string
  /** The stored name, when the server is being renamed. */
  readonly previousName?: string
  readonly server: {
    readonly name: string
    readonly command: string
    readonly args: readonly string[]
    readonly env: readonly SavedVariable[]
    readonly enabled: boolean
  }
}

export interface RemoveRequest {
  readonly revision: string
  readonly name: string
}

/** Which bound left an inspection incomplete. */
export type InspectCut = "tools" | "ui" | "bytes"

export interface InspectedTool {
  readonly name: string
  /** Present only when the server gave the hint as a boolean. */
  readonly readOnly?: boolean
  readonly destructive?: boolean
  readonly ui?: {
    readonly uri: string
    /** Each CSP list the app asks for, by its name, with its origins. */
    readonly csp: readonly {
      readonly name: string
      readonly origins: readonly string[]
    }[]
    /** The permissions it asks for, by name. */
    readonly permissions: readonly string[]
  }
}

export interface Inspection {
  readonly complete: boolean
  readonly cut?: InspectCut
  readonly tools: readonly InspectedTool[]
}

/** Why a saved server was refused as invalid, in the window's words for the gateway's problems. */
export type Problem =
  | "tooMany"
  | "duplicateName"
  | "name"
  | "command"
  | "arguments"
  | "environmentName"
  | "reservedEnvironmentName"
  | "environmentValue"
  | "environmentValueMissing"
  | "environmentNameRepeated"

/** The gateway's refusals of these methods, one for each of its codes. */
export type RefusalCode =
  | "notConfigured"
  | "invalid"
  | "reservedName"
  | "notFound"
  | "revisionConflict"
  | "busy"
  | "stopping"
  | "configInvalid"
  | "configTooLarge"
  | "storageUnavailable"
  | "auditUnavailable"
  | "startFailed"
  | "timedOut"
  | "gone"
  | "malformed"
  | "remoteError"

/**
 * How a request failed. `forbidden`: this credential may not manage servers.
 * `unanswered`: no answer this window can read — lost, timed out, or not a
 * refusal it knows — so a write may or may not have been applied.
 */
export type Failure =
  | { readonly kind: "forbidden" }
  | { readonly kind: "unanswered" }
  | {
      readonly kind: "invalid"
      readonly problem?: Problem
      /** The stored or saved server the problem is about. */
      readonly server?: string
      /** The variable the problem is about. Never a value. */
      readonly name?: string
    }
  | {
      readonly kind: "auditUnavailable"
      readonly applied?: boolean
      /** What stopped the request, as the gateway's code spells it, to show. */
      readonly code?: string
    }
  | { readonly kind: "remoteError"; readonly code?: number; readonly message?: string }
  | {
      readonly kind: Exclude<RefusalCode, "invalid" | "auditUnavailable" | "remoteError">
    }

type Invalid = Extract<Failure, { readonly kind: "invalid" }>

export type Outcome<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly failure: Failure }

/** The limits the window shows, as the protocol publishes them. */
export interface McpServersLimits {
  /** How long one inspection may run at the gateway. */
  readonly inspectDeadlineMs: number
}

/** A variable row in the form. A stored one keeps its value unless one is typed. */
export interface VariableRow {
  readonly key: number
  readonly name: string
  readonly value: string
  /** Listed with the server: its value is kept while this row's is empty. */
  readonly stored: boolean
}

export type FormField = "name" | "command" | "args" | "env" | "form"

export interface FormProblem {
  readonly field: FormField
  readonly text: string
}

export interface ServerForm {
  /** The stored name of the server being edited; absent while adding. */
  readonly editing?: string
  readonly name: string
  readonly command: string
  /** One argument per line. */
  readonly args: string
  readonly env: readonly VariableRow[]
  readonly enabled: boolean
  readonly problem?: FormProblem
}

/** The one request in flight. */
export type PendingRequest =
  | { readonly kind: "list"; readonly seq: number }
  | {
      readonly kind: "save"
      readonly seq: number
      readonly request: SaveRequest
      /** The server whose switch sent it, when a switch did. */
      readonly toggled?: string
    }
  | { readonly kind: "remove"; readonly seq: number; readonly request: RemoveRequest }

export type ListState =
  | { readonly phase: "loading" }
  | { readonly phase: "listed"; readonly list: ServerList }
  | { readonly phase: "notConfigured" }
  | { readonly phase: "failed" }

export type InspectionState =
  | { readonly phase: "running"; readonly seq: number; readonly name: string }
  | { readonly phase: "done"; readonly name: string; readonly result: Inspection }
  | { readonly phase: "failed"; readonly name: string; readonly text: string }

/**
 * What the last answer said. `list`: a list that failed, which the next list
 * that succeeds answers, so it goes then. `write`: a write's, kept until the
 * next action, since the list read after it does not say what it did.
 */
export interface Notice {
  readonly text: string
  readonly from: "list" | "write"
}

export interface McpServersState {
  readonly limits: McpServersLimits
  /** Whether this credential may manage servers; unknown until the gateway is reached. */
  readonly access: "unknown" | "admin" | "notAdmin"
  readonly connection: "connecting" | "connected" | "unreachable"
  readonly list: ListState
  readonly pending: PendingRequest | null
  readonly form: ServerForm | null
  /** The server whose removal is being asked about. */
  readonly confirming: string | null
  readonly inspection: InspectionState | null
  /** What the last answer said, when it said something. */
  readonly notice: Notice | null
  /** The last request number handed out. */
  readonly seq: number
  /** The last variable row key handed out. */
  readonly rows: number
}

export type McpServersEvent =
  | { readonly type: "connected"; readonly mayManage: boolean }
  | { readonly type: "unreachable" }
  | {
      readonly type: "answered"
      readonly seq: number
      readonly outcome: Outcome<unknown>
    }
  | { readonly type: "retry" }
  | { readonly type: "add" }
  | { readonly type: "edit"; readonly name: string }
  | {
      readonly type: "change"
      readonly patch: Partial<Pick<ServerForm, "name" | "command" | "args" | "enabled">>
    }
  | { readonly type: "addVariable" }
  | {
      readonly type: "changeVariable"
      readonly key: number
      readonly patch: Partial<Pick<VariableRow, "name" | "value">>
    }
  | { readonly type: "removeVariable"; readonly key: number }
  | { readonly type: "cancelForm" }
  | { readonly type: "save" }
  | { readonly type: "toggle"; readonly name: string }
  | { readonly type: "askRemove"; readonly name: string }
  | { readonly type: "cancelRemove" }
  | { readonly type: "confirmRemove" }
  | { readonly type: "inspect"; readonly name: string }
  | {
      readonly type: "inspected"
      readonly seq: number
      readonly outcome: Outcome<Inspection>
    }
  | { readonly type: "closeInspection" }

export function initialMcpServersState(limits: McpServersLimits): McpServersState {
  return {
    limits,
    access: "unknown",
    connection: "connecting",
    list: { phase: "loading" },
    pending: null,
    form: null,
    confirming: null,
    inspection: null,
    notice: null,
    seq: 0,
    rows: 0,
  }
}

/* ——— What the window says ——— */

const quoted = (name: string) => `“${name}”`
const seconds = (ms: number) => `${Math.round(ms / 1000)} seconds`

export const sentences = {
  notAdmin: "Only an administrator can manage MCP servers.",
  unreachable: "Gateway unreachable",
  notConfigured: "This gateway isn't managing MCP servers.",
  empty: "No servers yet",
  managed: "Managed by Nessa",
  storedValue: "Stored value kept",
  listFailed: "The servers couldn't be listed.",
  conflict: "Changed elsewhere, the list was reloaded. Check and try again.",
  notFound: "That server is no longer stored. The list was reloaded.",
  busy: "Another change is in progress. Try again in a moment.",
  stopping: "The gateway is stopping, so nothing was changed.",
  /** Named from the list's managed row, the one the gateway keeps for itself. */
  reservedName: (name: string | undefined) =>
    name === undefined
      ? "That name is Nessa's own server's, and can't be changed here."
      : `${quoted(name)} is Nessa's own server, and can't be changed here.`,
  configInvalid: "The configuration file can't be read as it is, so nothing was changed.",
  configTooLarge: "The configuration would be too large, so nothing was changed.",
  storageUnavailable:
    "The configuration file couldn't be read or written, so nothing was changed.",
  unanswered: "Not confirmed. The list shows where things stand.",
  forbidden: "Only an administrator can manage MCP servers.",
  gone: (name: string) => `${quoted(name)} is no longer stored.`,
  tools: (count: number) =>
    count === 0
      ? "It offers no tools."
      : count === 1
        ? "It offers 1 tool."
        : `It offers ${count} tools.`,
  removeAsk: (name: string) =>
    `Remove ${quoted(name)}? New conversations stop getting it. Open ones keep it until they close.`,
  inspecting: (name: string, ms: number) =>
    `Starting ${quoted(name)}… It has up to ${seconds(ms)}.`,
  variables: (count: number) => (count === 1 ? "1 variable" : `${count} variables`),
  cut: {
    tools: "The server offered more tools than are read; the rest aren't listed.",
    ui: "More apps than are read; the tools after them are listed without one.",
    bytes: "The answer was too long; tools were left off the end.",
  } satisfies Record<InspectCut, string>,
} as const

function auditSentence(
  failure: Extract<Failure, { kind: "auditUnavailable" }>,
  what: "change" | "inspection",
) {
  const code = failure.code ? ` (${failure.code})` : ""
  const done =
    failure.applied === undefined
      ? `Whether the ${what} happened isn't known`
      : failure.applied
        ? what === "change"
          ? "The change was made"
          : "The server was started"
        : what === "change"
          ? "Nothing was changed"
          : "The server wasn't started"
  return `${done}${code}, but it couldn't be recorded. The list was reloaded.`
}

/** What a problem says, at the field it is about. Never the rule's pattern: what is wrong. */
function problemAt(failure: Invalid): FormProblem {
  const { problem, server, name } = failure
  const it = name === undefined ? "A variable" : quoted(name)
  switch (problem) {
    case "tooMany":
      return { field: "form", text: "There are too many servers. Remove one first." }
    case "duplicateName":
      return {
        field: "name",
        text: `Another server is named ${quoted(server ?? "that")}.`,
      }
    case "name":
      return { field: "name", text: "This name can't be used for a server." }
    case "command":
      return { field: "command", text: "This command can't be used." }
    case "arguments":
      return { field: "args", text: "One of the arguments can't be used." }
    case "environmentName":
      return { field: "env", text: `${it} isn't a name a variable can have.` }
    case "reservedEnvironmentName":
      return { field: "env", text: `${it} is reserved for Nessa.` }
    case "environmentValue":
      return { field: "env", text: `The value of ${it} can't be used.` }
    case "environmentValueMissing":
      return { field: "env", text: `${it} has no stored value. Enter one.` }
    case "environmentNameRepeated":
      return { field: "env", text: `${it} is given twice.` }
    case undefined:
      return { field: "form", text: "The gateway refused this server as it is." }
  }
}

/** What a failed inspection says. */
function inspectSentence(name: string, failure: Failure, state: McpServersState) {
  switch (failure.kind) {
    case "startFailed":
      return `${quoted(name)} couldn't be started. Check its command.`
    case "timedOut":
      return `${quoted(name)} didn't finish within ${seconds(state.limits.inspectDeadlineMs)}.`
    case "gone":
      return `${quoted(name)} stopped before it answered.`
    case "malformed":
      return `${quoted(name)} answered with something that isn't MCP.`
    case "remoteError":
      return failure.message === undefined
        ? `${quoted(name)} answered with an error.`
        : `${quoted(name)} answered with an error: ${failure.message}${failure.code === undefined ? "" : ` (${failure.code})`}`
    case "busy":
      return "Other servers are being inspected. Try again in a moment."
    case "stopping":
      return `The gateway is stopping, so ${quoted(name)} wasn't started.`
    case "auditUnavailable":
      return auditSentence(failure, "inspection")
    default:
      return writeSentence(failure, state)
  }
}

/** What a failed list, save or remove says when it is not at a field. */
function writeSentence(failure: Failure, state: McpServersState): string {
  switch (failure.kind) {
    case "notConfigured":
      return sentences.notConfigured
    case "reservedName":
      return sentences.reservedName(managedName(state))
    case "notFound":
      return sentences.notFound
    case "revisionConflict":
      return sentences.conflict
    case "busy":
      return sentences.busy
    case "stopping":
      return sentences.stopping
    case "configInvalid":
      return sentences.configInvalid
    case "configTooLarge":
      return sentences.configTooLarge
    case "storageUnavailable":
      return sentences.storageUnavailable
    case "auditUnavailable":
      return auditSentence(failure, "change")
    case "forbidden":
      return sentences.forbidden
    case "invalid": {
      // Away from the form, the server is named: it may be one stored by hand.
      const { text } = problemAt(failure)
      return failure.server === undefined ? text : `${quoted(failure.server)}: ${text}`
    }
    case "startFailed":
    case "timedOut":
    case "gone":
    case "malformed":
    case "remoteError":
    case "unanswered":
      return sentences.unanswered
  }
}

/* ——— What may be done now ——— */

/** Whether the tab may send a write now: one request at a time, on a list, while reachable. */
export function canWrite(state: McpServersState): boolean {
  return (
    state.access === "admin" &&
    state.connection === "connected" &&
    state.pending === null &&
    state.list.phase === "listed"
  )
}

/**
 * Whether an inspection may start now: one at a time, while reachable, and
 * not while a write is in flight (U21).
 */
export function canInspect(state: McpServersState): boolean {
  return (
    state.access === "admin" &&
    state.connection === "connected" &&
    state.pending === null &&
    state.list.phase === "listed" &&
    state.inspection?.phase !== "running"
  )
}

/** Whether the form holds what a save needs before the gateway can judge it: a non-blank name and command (U7). */
export function formReady(form: ServerForm): boolean {
  return form.name.trim() !== "" && form.command.trim() !== ""
}

/** The servers the form edits, and the managed one, from a list. */
export function storedServers(list: ServerList) {
  return {
    stored: list.servers.filter((server) => !server.managed),
    managed: list.servers.filter((server) => server.managed),
  }
}

/* ——— The requests ——— */

/** One argument per line; a final line break is not an argument of its own. */
export function argsOf(text: string): string[] {
  if (text === "") return []
  return (text.endsWith("\n") ? text.slice(0, -1) : text).split("\n")
}

/**
 * The save a form asks for. A stored variable left empty keeps its value
 * (`null`); a row with neither name nor value is a blank one and is not sent.
 */
export function saveRequestOf(form: ServerForm, revision: string): SaveRequest {
  const renamed = form.editing !== undefined && form.editing !== form.name
  return {
    revision,
    ...(renamed ? { previousName: form.editing } : {}),
    server: {
      name: form.name,
      command: form.command,
      args: argsOf(form.args),
      env: form.env
        .filter((row) => row.stored || row.name !== "" || row.value !== "")
        .map((row) => ({
          name: row.name,
          value: row.stored && row.value === "" ? null : row.value,
        })),
      enabled: form.enabled,
    },
  }
}

/** The save a switch sends: the server as listed, every value kept, the other way on. */
export function toggleRequestOf(server: ListedServer, revision: string): SaveRequest {
  return {
    revision,
    server: {
      name: server.name,
      command: server.command,
      args: [...server.args],
      env: server.envNames.map((name) => ({ name, value: null })),
      enabled: !server.enabled,
    },
  }
}

/* ——— The reducer ——— */

function listAgain(state: McpServersState): McpServersState {
  const seq = state.seq + 1
  return { ...state, seq, pending: { kind: "list", seq } }
}

function listed(state: McpServersState): ServerList | undefined {
  return state.list.phase === "listed" ? state.list.list : undefined
}

function server(state: McpServersState, name: string): ListedServer | undefined {
  return listed(state)?.servers.find((each) => each.name === name)
}

/** The name of the managed server, as the list last said. */
function managedName(state: McpServersState): string | undefined {
  return listed(state)?.servers.find((each) => each.managed)?.name
}

const said = (text: string, from: Notice["from"]): Notice => ({ text, from })

/**
 * A finished inspection of a server the list no longer has — removed or
 * renamed while it ran, or since — says so, rather than show what it was.
 */
function inspectionOfStored(state: McpServersState): McpServersState {
  const inspection = state.inspection
  if (!inspection || inspection.phase === "running" || !listed(state)) return state
  if (server(state, inspection.name)) return state
  return {
    ...state,
    inspection: {
      phase: "failed",
      name: inspection.name,
      text: sentences.gone(inspection.name),
    },
  }
}

function editForm(state: McpServersState, name: string): McpServersState {
  const found = server(state, name)
  if (!found || found.managed) return state
  let rows = state.rows
  const env = found.envNames.map((variable) => ({
    key: ++rows,
    name: variable,
    value: "",
    stored: true,
  }))
  return {
    ...state,
    rows,
    notice: null,
    confirming: null,
    form: {
      editing: found.name,
      name: found.name,
      command: found.command,
      args: found.args.join("\n"),
      env,
      enabled: found.enabled,
    },
  }
}

/** A forbidden answer: this credential may not manage servers, whatever was shown. */
function forbidden(state: McpServersState): McpServersState {
  return {
    ...state,
    access: "notAdmin",
    pending: null,
    form: null,
    confirming: null,
    inspection: null,
    notice: null,
  }
}

function answeredList(
  state: McpServersState,
  outcome: Outcome<unknown>,
): McpServersState {
  const done = { ...state, pending: null }
  if (outcome.ok)
    return inspectionOfStored({
      ...done,
      list: { phase: "listed", list: outcome.value as ServerList },
      // This list answers a list that failed; a write's notice stands.
      notice: state.notice?.from === "list" ? null : state.notice,
    })
  const { failure } = outcome
  if (failure.kind === "forbidden") return forbidden(state)
  if (failure.kind === "notConfigured")
    return { ...done, list: { phase: "notConfigured" }, form: null, notice: null }
  return {
    ...done,
    list: { phase: "failed" },
    notice: said(
      failure.kind === "unanswered"
        ? sentences.listFailed
        : writeSentence(failure, state),
      "list",
    ),
  }
}

function answeredWrite(
  state: McpServersState,
  pending: Extract<PendingRequest, { kind: "save" | "remove" }>,
  outcome: Outcome<unknown>,
): McpServersState {
  const done = { ...state, pending: null }
  const fromForm = pending.kind === "save" && pending.toggled === undefined
  if (outcome.ok)
    return listAgain({
      ...done,
      notice: null,
      form: fromForm ? null : state.form,
      confirming: pending.kind === "remove" ? null : state.confirming,
    })
  const { failure } = outcome
  switch (failure.kind) {
    case "forbidden":
      return forbidden(state)
    case "notConfigured":
      return { ...done, list: { phase: "notConfigured" }, form: null, notice: null }
    case "invalid":
      // At its field, the form kept, nothing listed again: nothing changed. A
      // problem with another stored server (one edited in by hand) is not the
      // form's: it is said, naming that server, and the form is kept.
      return fromForm &&
        state.form &&
        (failure.server === undefined || failure.server === pending.request.server.name)
        ? {
            ...done,
            notice: null,
            form: { ...state.form, problem: problemAt(failure) },
          }
        : { ...done, notice: said(writeSentence(failure, state), "write") }
    case "busy":
    case "stopping":
    case "reservedName":
      // Nothing was written; the controls come back as they were.
      return { ...done, notice: said(writeSentence(failure, state), "write") }
    case "auditUnavailable":
      return listAgain({
        ...done,
        notice: said(writeSentence(failure, state), "write"),
        form: failure.applied === true && fromForm ? null : state.form,
        confirming: null,
      })
    default:
      // The list shows where things stand: what was typed is kept to try again.
      return listAgain({
        ...done,
        notice: said(writeSentence(failure, state), "write"),
        confirming: null,
      })
  }
}

export function mcpServersReducer(
  state: McpServersState,
  event: McpServersEvent,
): McpServersState {
  switch (event.type) {
    case "connected": {
      const next: McpServersState = {
        ...state,
        connection: "connected",
        access: event.mayManage ? "admin" : "notAdmin",
      }
      if (!event.mayManage) return forbidden(next)
      // Listed again on every connection: what changed while away is read.
      return next.pending === null ? listAgain(next) : next
    }
    case "unreachable":
      return { ...state, connection: "unreachable" }
    case "answered": {
      const pending = state.pending
      if (pending === null || pending.seq !== event.seq) return state
      return pending.kind === "list"
        ? answeredList(state, event.outcome)
        : answeredWrite(state, pending, event.outcome)
    }
    case "retry":
      if (state.access !== "admin" || state.connection !== "connected" || state.pending)
        return state
      return listAgain({ ...state, notice: null })
    case "add":
      if (!canWrite(state)) return state
      return {
        ...state,
        notice: null,
        confirming: null,
        form: { name: "", command: "", args: "", env: [], enabled: true },
      }
    case "edit":
      return canWrite(state) ? editForm(state, event.name) : state
    case "change":
      if (!state.form || state.pending) return state
      return { ...state, form: { ...state.form, ...event.patch } }
    case "addVariable":
      if (!state.form || state.pending) return state
      return {
        ...state,
        rows: state.rows + 1,
        form: {
          ...state.form,
          env: [
            ...state.form.env,
            { key: state.rows + 1, name: "", value: "", stored: false },
          ],
        },
      }
    case "changeVariable":
      if (!state.form || state.pending) return state
      return {
        ...state,
        form: {
          ...state.form,
          env: state.form.env.map((row) =>
            row.key !== event.key
              ? row
              : // A stored variable's name is the one stored; only its value changes.
                { ...row, ...event.patch, ...(row.stored ? { name: row.name } : {}) },
          ),
        },
      }
    case "removeVariable":
      if (!state.form || state.pending) return state
      return {
        ...state,
        form: {
          ...state.form,
          env: state.form.env.filter((row) => row.key !== event.key),
        },
      }
    case "cancelForm":
      if (state.pending) return state
      return { ...state, form: null }
    case "save": {
      const list = listed(state)
      if (!state.form || !list || !canWrite(state) || !formReady(state.form)) return state
      const seq = state.seq + 1
      const form = { ...state.form, problem: undefined }
      return {
        ...state,
        seq,
        notice: null,
        form,
        pending: { kind: "save", seq, request: saveRequestOf(form, list.revision) },
      }
    }
    case "toggle": {
      const list = listed(state)
      const found = server(state, event.name)
      if (!list || !found || found.managed || !canWrite(state)) return state
      const seq = state.seq + 1
      return {
        ...state,
        seq,
        notice: null,
        pending: {
          kind: "save",
          seq,
          toggled: found.name,
          request: toggleRequestOf(found, list.revision),
        },
      }
    }
    case "askRemove": {
      const found = server(state, event.name)
      if (!found || found.managed || !canWrite(state)) return state
      return { ...state, confirming: found.name, notice: null }
    }
    case "cancelRemove":
      if (state.pending) return state
      return { ...state, confirming: null }
    case "confirmRemove": {
      const list = listed(state)
      if (!list || state.confirming === null || !canWrite(state)) return state
      const seq = state.seq + 1
      return {
        ...state,
        seq,
        pending: {
          kind: "remove",
          seq,
          request: { revision: list.revision, name: state.confirming },
        },
      }
    }
    case "inspect": {
      const found = server(state, event.name)
      if (!found || found.managed || !canInspect(state)) return state
      const seq = state.seq + 1
      return { ...state, seq, inspection: { phase: "running", seq, name: found.name } }
    }
    case "inspected": {
      const running = state.inspection
      if (running?.phase !== "running" || running.seq !== event.seq) return state
      const { outcome } = event
      if (outcome.ok)
        return inspectionOfStored({
          ...state,
          inspection: { phase: "done", name: running.name, result: outcome.value },
        })
      if (outcome.failure.kind === "forbidden") return forbidden(state)
      const failed: McpServersState = {
        ...state,
        inspection: {
          phase: "failed",
          name: running.name,
          text: inspectSentence(running.name, outcome.failure, state),
        },
      }
      // A server no longer stored: the list shows where things stand.
      return outcome.failure.kind === "notFound" && failed.pending === null
        ? listAgain(failed)
        : failed
    }
    case "closeInspection":
      if (state.inspection?.phase === "running") return state
      return { ...state, inspection: null }
  }
}
