/**
 * Settings › Connections › Integrations: the gateway's stored MCP servers, as
 * the window manages them (#391). One reducer, from the design's state table
 * (rows U1–U31, issue #391's PR 3 design, U32–U43 from its review,
 * U44–U50 for a list too large to show, #391 comment 5986496625, S1–S8,
 * G1–G9 and F8–F12 for the secret field and rows sharing a name, #391
 * comment 5987640015, and V1–V10, L1–L6 and M1–M6 for the kept-value rule
 * and a write's `live`, #391 comment 5988122201); its tests are one row at least one test, in
 * `mcp-servers.test.ts`.
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
  /** The remote endpoint, when this row is a remote server. */
  readonly url?: string
  /** The remote server's durable id, when this row is remote. */
  readonly remoteId?: string
  /** Redacted authorization facts, when the gateway has a record. */
  readonly authorization?: AuthorizationFacts
}

/** Where a remote server's authorization stands, with no token in it. */
export interface AuthorizationFacts {
  readonly phase: string
  readonly tokenExpired: boolean
  readonly refreshFailing: boolean
  readonly scopeRequired: boolean
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

export type SavedServer =
  | {
      readonly kind: "stdio"
      readonly name: string
      readonly command: string
      readonly args: readonly string[]
      readonly env: readonly SavedVariable[]
      readonly enabled: boolean
    }
  | {
      readonly kind: "remote"
      readonly name: string
      readonly url: string
      readonly enabled: boolean
    }

export interface SaveRequest {
  readonly revision: string
  /** The stored name, when the server is being renamed. */
  readonly previousName?: string
  readonly server: SavedServer
}

/** What mcpServers.authorize answered. No token is in it. */
export interface AuthorizeResult {
  readonly status: "not_required" | "pending_consent" | "ready"
  readonly consentUrl?: string
}

/** What mcpServers.revoke answered. Incomplete stays incomplete. */
export interface RevokeResult {
  readonly settled: boolean
}

export interface RemoveRequest {
  readonly revision: string
  readonly name: string
}

/**
 * What a save or remove answered. `live`: whether new conversations get the
 * list as now written. False when the gateway is stopping, or when a remove
 * left a list edited by hand still past a bound (the removed server is out
 * all the same, and the rest stay as they were).
 */
export interface WriteResult {
  readonly live: boolean
}

/**
 * Which bound left an inspection incomplete. `stopping`: the gateway began to
 * stop after the server was started, so no tools were read.
 */
export type InspectCut = "tools" | "ui" | "bytes" | "stopping"

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
  | "url"
  | "duplicateServerId"

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
  | "unreachable"
  | "unauthorized"
  | "insufficientScope"
  | "sessionCollision"
  | "authorizationHeld"
  | "storeUnavailable"
  | "registrationUnsupported"
  | "discoveryFailed"
  | "authorizationIncomplete"

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
      /** Whether the change was published, or the inspected server started. */
      readonly applied?: boolean
      /** What stopped the request, or what it would have been answered, in the window's words. */
      readonly cause?: RefusalCode
    }
  | {
      readonly kind: "storageUnavailable"
      /** true: published, the live set following as far as it could, but it may not survive a crash. false: nothing was written. */
      readonly applied?: boolean
    }
  | { readonly kind: "remoteError"; readonly code?: number; readonly message?: string }
  | {
      readonly kind: "configTooLarge"
      /**
       * Only on a list whose stored servers would not fit one frame: the
       * revision a remove by name must name (U44). Absent on a save, and on
       * a configuration file itself too large to read (U47).
       */
      readonly revision?: string
    }
  | {
      readonly kind: Exclude<
        RefusalCode,
        | "invalid"
        | "auditUnavailable"
        | "storageUnavailable"
        | "remoteError"
        | "configTooLarge"
      >
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

/**
 * A variable row in the form. A stored one keeps its value until its value
 * field is edited; edited and left empty, it saves the empty value (S1, S2).
 */
export interface VariableRow {
  readonly key: number
  readonly name: string
  readonly value: string
  /** Listed with the server: its value is kept while this row's is not edited. */
  readonly stored: boolean
  /** Whether the value field was typed in, pasted into, cleared or trimmed. */
  readonly edited: boolean
  /**
   * The value came from a paste with a line break, which a password field
   * cannot hold: it is held here and never drawn (S4).
   */
  readonly held: boolean
}

/** An argument row in the form: one argument, any text, the empty one and line breaks included. */
export interface ArgumentRow {
  readonly key: number
  readonly value: string
}

export type FormField = "name" | "command" | "args" | "env" | "url" | "form"

export interface FormProblem {
  readonly field: FormField
  readonly text: string
}

export interface ServerForm {
  /** The stored name of the server being edited; absent while adding. */
  readonly editing?: string
  /**
   * The server as listed when the form was filled, or last refilled: a field
   * still equal to it is untouched, and follows the list (U42).
   */
  readonly base?: ListedServer
  readonly name: string
  readonly command: string
  readonly args: readonly ArgumentRow[]
  readonly env: readonly VariableRow[]
  readonly enabled: boolean
  /** How this server is reached. An edit keeps the kind it was listed with. */
  readonly kind: "stdio" | "remote"
  /** The remote endpoint, while kind is remote. */
  readonly url: string
  readonly problem?: FormProblem
  /**
   * Its save's outcome is not known (unanswered, or not said whether it was
   * applied): the list read next may no longer have the name it edits, and
   * that is the save's doing, not news to report (F9).
   */
  readonly unconfirmed?: true
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
  | {
      readonly kind: "authorize"
      readonly seq: number
      readonly id: string
      readonly name: string
      readonly revision: string
    }
  | {
      readonly kind: "revoke"
      readonly seq: number
      readonly id: string
      readonly name: string
      readonly revision: string
    }

export type ListState =
  | { readonly phase: "loading" }
  | {
      readonly phase: "listed"
      readonly list: ServerList
      /** Each listed server's occurrence id, by its place in `list.servers` (G1). */
      readonly ids: readonly number[]
    }
  | { readonly phase: "notConfigured" }
  | { readonly phase: "failed" }
  /**
   * The stored servers would not fit one answer (U44). The refusal names no
   * server, so none is shown: a server is removed by the name typed here, at
   * the refusal's revision.
   */
  | { readonly phase: "tooLarge"; readonly revision: string; readonly name: string }

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

/**
 * The removal being asked about: the name the request will carry, and how
 * many stored servers had it when asked — the gateway removes the first
 * stored under it, so a confirm stands only while that count does (G7).
 * `count` is `null` for a name typed against a list too large to show (U44).
 */
export interface Confirming {
  readonly name: string
  readonly count: number | null
}

export interface McpServersState {
  readonly limits: McpServersLimits
  /**
   * Whether this credential may manage servers, as the gateway last answered:
   * a list it gave, or refused as too large to show, says yes; a `forbidden`
   * refusal says no. Which grant that takes is the gateway's; the window
   * never reads grants.
   */
  readonly access: "unknown" | "admin" | "notAdmin"
  readonly connection: "connecting" | "connected" | "unreachable"
  readonly list: ListState
  readonly pending: PendingRequest | null
  readonly form: ServerForm | null
  /** The removal being asked about. */
  readonly confirming: Confirming | null
  readonly inspection: InspectionState | null
  /** A consent page the host should open, for the named remote. */
  readonly consent: { readonly name: string; readonly url: string } | null
  /** What the last answer said, when it said something. */
  readonly notice: Notice | null
  /** The last request number handed out. */
  readonly seq: number
  /** The last row key, or listed server's occurrence id, handed out. */
  readonly rows: number
}

export type McpServersEvent =
  | { readonly type: "connected" }
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
      readonly patch: Partial<
        Pick<ServerForm, "name" | "command" | "enabled" | "kind" | "url">
      >
    }
  /** A new argument, after the row keyed `after`, or last. */
  | { readonly type: "addArgument"; readonly after?: number }
  | { readonly type: "changeArgument"; readonly key: number; readonly value: string }
  | { readonly type: "removeArgument"; readonly key: number }
  | { readonly type: "addVariable" }
  | {
      readonly type: "changeVariable"
      readonly key: number
      readonly patch: Partial<Pick<VariableRow, "name" | "value">>
    }
  /** A paste with a line break: held, never drawn (S4). */
  | { readonly type: "pasteVariable"; readonly key: number; readonly value: string }
  /** One trailing line break of a held value removed (S5). */
  | { readonly type: "trimVariable"; readonly key: number }
  /** A held value dropped: the field is empty and edited (S6). */
  | { readonly type: "clearVariable"; readonly key: number }
  /** A stored variable back to keeping its stored value (S2 → S1). */
  | { readonly type: "keepVariable"; readonly key: number }
  | { readonly type: "removeVariable"; readonly key: number }
  | { readonly type: "cancelForm" }
  | { readonly type: "save" }
  | { readonly type: "toggle"; readonly name: string }
  /** Remove asked for `name`: its row's, or the group's when it is shared (G5). */
  | { readonly type: "askRemove"; readonly name: string }
  | { readonly type: "cancelRemove" }
  /** The name typed to remove a server from a list too large to show (U44). */
  | { readonly type: "changeRemoveName"; readonly name: string }
  | { readonly type: "askRemoveByName" }
  | { readonly type: "confirmRemove" }
  | { readonly type: "authorize"; readonly name: string }
  | { readonly type: "revoke"; readonly name: string }
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
    consent: null,
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
  /** A stored variable's placeholder once the launch changed (U33, V2–V5). */
  storedValueAgain: "Enter the value again",
  valuesAgain:
    "Changing the command, arguments or variables needs every stored value entered again.",
  /** A save or switch the gateway stored while stopping (L2, L3). */
  notLiveSaved: (what: "save" | "change") =>
    `${what === "save" ? "Saved" : "Changed"}. The gateway is stopping, so new conversations get it once it starts again.`,
  /** A remove whose resulting list did not go live as a whole (L5). */
  notLiveRemoved:
    "Removed. The rest of the list takes effect once the configuration is fixed, or the gateway starts again.",
  listFailed: "The servers couldn't be listed.",
  /** A list refused for a reason a list can have (F10). */
  listRefused: (why: string) => `The servers couldn't be listed: ${why}.`,
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
  /** A list whose stored servers would not fit one answer (U44). */
  listTooLarge:
    "The server list is too large to show. Removing a server fixes it: enter its name.",
  /** A list refused as too large with no revision to remove at (U47). */
  configFileTooLarge: "The configuration file is too large to read here.",
  authorizationHeld:
    "Saved, and the previous authorization is still in place. The new address is not live until that authorization is revoked.",
  storeUnavailable: "This computer cannot store a token, so authorization did not start.",
  registrationUnsupported:
    "That server does not offer registration, so it cannot be authorized here.",
  discoveryFailed: "Authorization did not finish. No token was stored.",
  authorizationIncomplete:
    "Authorization was sent and the answer was not kept. Authorize again before using the server.",
  consentNeeded: "Consent needed",
  pendingConsent: "Waiting for consent",
  tokenExpired: "Token expired",
  refreshFailing: "Refresh failing",
  scopeRequired: "Scope required",
  revocationIncomplete: "Revocation incomplete",
  authorizeReady: "Authorized",
  authorizeNotRequired: "This server does not require authorization.",
  revokeSettled: "Authorization revoked.",
  revokeIncomplete: "Revocation is incomplete.",
  /** A save whose resulting list would not fit (U48, U49). */
  saveTooLarge:
    "This would make the server list too large; remove a server or shorten its arguments.",
  /** A remove by name of a name not stored (U46). */
  noSuchServer: (name: string) => `No server is stored under ${quoted(name)}.`,
  storageUnavailable:
    "The configuration file couldn't be read or written, so nothing was changed.",
  storageUnknown:
    "The configuration file couldn't be read or written. The list shows where things stand.",
  /** Published, but its directory not synced (U36). */
  notDurable: (what: "save" | "remove" | "change") =>
    `${what === "save" ? "Saved" : what === "remove" ? "Removed" : "Changed"}, but it may not survive a crash.`,
  inspectUnanswered: (name: string) =>
    `${quoted(name)}'s inspection didn't answer in time, or the connection was lost.`,
  formGone: (name: string) =>
    `${quoted(name)} is no longer stored, so the form was closed.`,
  changedThere: (fields: readonly string[]) =>
    `Changed elsewhere too, and kept as typed here: ${fields.join(", ")}.`,
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
  /** A group's one action: the gateway removes by name, the first stored under it (G3). */
  removeFirst: (name: string) => `Remove the first server named ${quoted(name)}`,
  /** Names the first one's command: the server the gateway will remove (M5). */
  removeFirstAsk: (name: string, command: string) =>
    `Remove the first server named ${quoted(name)}, which runs ${command}? New conversations stop getting it. Open ones keep it until they close.`,
  /** A name typed against a list too large to show may be stored more than once (G9). */
  removeByNameAsk: (name: string) =>
    `Remove ${quoted(name)}? This removes the first server stored under that name. New conversations stop getting it. Open ones keep it until they close.`,
  nameShared: (count: number) =>
    `${count} servers share this name. Only the first can be removed here, and none edited.`,
  /** A form whose server's name a list read since stores more than once (G8). */
  formShared: (name: string) =>
    `${quoted(name)} is now stored more than once, so the form was closed.`,
  notOffered: "Not offered",
  /** A stored variable's placeholder once its value was edited to empty (S2). */
  storedValueCleared: "Empty: the stored value will be cleared",
  keepStored: "Keep stored value",
  pasted: (lines: number) => `Pasted value: ${lines === 1 ? "1 line" : `${lines} lines`}`,
  endsWithBreak: "ends with a line break",
  trimBreak: "Remove line break",
  clearPasted: "Clear",
  /** The marker of an argument holding a line break (F11). */
  argumentBreaks: (count: number) =>
    count === 1 ? "Has a line break" : `Has ${count} line breaks`,
  inspecting: (name: string, ms: number) =>
    `Starting ${quoted(name)}… It has up to ${seconds(ms)}.`,
  variables: (count: number) => (count === 1 ? "1 variable" : `${count} variables`),
  cut: {
    tools: "The server offered more tools than are read; the rest aren't listed.",
    ui: "More apps than are read; the tools after them are listed without one.",
    bytes: "The answer was too long; tools were left off the end.",
    stopping:
      "The gateway began to stop, so the server was stopped before its tools were read.",
  } satisfies Record<InspectCut, string>,
} as const

/**
 * What stopped a request whose record could not be written, as a clause:
 * total over the refusals, so one the protocol adds is a type error here, not
 * a wire code shown (U43). `auditUnavailable` is never one, says the schema.
 */
const causes: Record<RefusalCode, string> = {
  notConfigured: "MCP servers aren't managed here",
  invalid: "the server was refused as it is",
  reservedName: "the name is Nessa's own server's",
  notFound: "the server is no longer stored",
  revisionConflict: "it was changed elsewhere",
  busy: "another change was in progress",
  stopping: "the gateway was stopping",
  configInvalid: "the configuration file can't be read as it is",
  configTooLarge: "the configuration would be too large",
  storageUnavailable: "the configuration file couldn't be read or written",
  auditUnavailable: "nothing could be recorded",
  startFailed: "the server couldn't be started",
  timedOut: "the server didn't finish in time",
  gone: "the server stopped before it answered",
  malformed: "the server's answer isn't MCP",
  remoteError: "the server answered with an error",
  unreachable: "the server could not be reached",
  unauthorized: "the server refused the caller",
  insufficientScope: "the authorization is not broad enough",
  sessionCollision: "the server is already open under another session",
  authorizationHeld: "the previous authorization is still in place",
  storeUnavailable: "this computer cannot store a token",
  registrationUnsupported: "the server does not offer registration",
  discoveryFailed: "authorization did not finish",
  authorizationIncomplete: "the authorization answer was not kept",
}

function auditSentence(
  failure: Extract<Failure, { kind: "auditUnavailable" }>,
  what: "change" | "inspection",
  name?: string,
) {
  const { applied, cause } = failure
  const why =
    cause === undefined
      ? ""
      : cause === "storageUnavailable" && applied === true
        ? " (it may not survive a crash)"
        : ` (${causes[cause]})`
  if (what === "inspection") {
    const it = quoted(name ?? "The server")
    // An inspection lists nothing again: no reload is claimed.
    return applied === undefined
      ? `Whether ${it} started isn't known, and the inspection couldn't be recorded${why}.`
      : applied
        ? `${it} may have started, but the inspection couldn't be recorded${why}.`
        : `${it} wasn't started, and the inspection couldn't be recorded${why}.`
  }
  const done =
    applied === undefined
      ? "Whether the change happened isn't known"
      : applied
        ? "The change was made"
        : "Nothing was changed"
  return `${done}, but it couldn't be recorded${why}. The list was reloaded.`
}

/** What a problem says, at the field it is about. Never the rule's pattern: what is wrong. */
function problemAt(failure: Invalid): FormProblem {
  const { problem, server, name } = failure
  // A variable with no name, or an empty one, is "A variable", never “”.
  const it = name ? quoted(name) : "A variable"
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
    case "url":
      return { field: "url", text: "This address can't be used for a server." }
    case "duplicateServerId":
      return { field: "form", text: "Two servers are stored with the same id." }
    case undefined:
      return { field: "form", text: "The gateway refused this server as it is." }
  }
}

/** What a failed inspection says. */
function inspectSentence(name: string, failure: Failure, state: McpServersState) {
  switch (failure.kind) {
    case "unreachable":
      return `${quoted(name)} could not be reached.`
    case "unauthorized":
      return `${quoted(name)} refused the caller.`
    case "insufficientScope":
      return `${quoted(name)} needs a broader authorization.`
    case "sessionCollision":
      return `${quoted(name)} is already open under another session.`
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
      return auditSentence(failure, "inspection", name)
    case "unanswered":
      return sentences.inspectUnanswered(name)
    default:
      return writeSentence(failure, state)
  }
}

/**
 * What a failed list says (F10): that it failed, and why when a refusal
 * says; never what a write's refusal says of a change. A file too large to
 * read is its own (U47).
 */
function listSentence(failure: Failure, state: McpServersState): string {
  switch (failure.kind) {
    case "configTooLarge":
      return sentences.configFileTooLarge
    case "forbidden":
    case "notConfigured":
    case "reservedName":
      return writeSentence(failure, state)
    case "unanswered":
    case "invalid":
    case "remoteError":
    case "auditUnavailable":
      return sentences.listFailed
    default:
      return sentences.listRefused(causes[failure.kind])
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
      return failure.applied === undefined
        ? sentences.storageUnknown
        : failure.applied
          ? sentences.notDurable("change")
          : sentences.storageUnavailable
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
    case "unreachable":
    case "unauthorized":
    case "insufficientScope":
    case "sessionCollision":
    case "unanswered":
      return sentences.unanswered
    case "authorizationHeld":
      return sentences.authorizationHeld
    case "storeUnavailable":
      return sentences.storeUnavailable
    case "registrationUnsupported":
      return sentences.registrationUnsupported
    case "discoveryFailed":
      return sentences.discoveryFailed
    case "authorizationIncomplete":
      return sentences.authorizationIncomplete
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

/**
 * Whether a server may be removed by a typed name now: the list too large to
 * show (U44), one request at a time, while reachable, a name typed.
 */
export function canRemoveByName(state: McpServersState): boolean {
  return (
    state.access === "admin" &&
    state.connection === "connected" &&
    state.pending === null &&
    state.list.phase === "tooLarge" &&
    state.list.name !== ""
  )
}

const sameArgs = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((each, at) => each === b[at])

const argsOf = (form: ServerForm) => form.args.map((row) => row.value)

/** A row a save sends: a stored one, or one with a name or a value (V8). */
const sent = (row: VariableRow) => row.stored || row.name !== "" || row.value !== ""

/**
 * Whether the form keeps the server's command, arguments and variable names
 * as listed: none added, none removed (V1, V9). Only then may a stored value
 * be kept, and only then is "Keep stored value" offered.
 */
export function namesKept(form: ServerForm, listed: ListedServer | undefined): boolean {
  if (form.editing === undefined || listed === undefined) return true
  const stored = form.env.filter((row) => row.stored).map((row) => row.name)
  return (
    form.command === listed.command &&
    sameArgs(argsOf(form), listed.args) &&
    form.env.every((row) => row.stored || !sent(row)) &&
    listed.envNames.every((name) => stored.includes(name))
  )
}

/**
 * Whether the form launches the server it edits otherwise than the list
 * says: another command, other arguments, a variable added or removed, or a
 * stored value edited, which the window can't compare with the one stored.
 * The gateway keeps a stored value only for the same launch (U33, V2–V5),
 * so then every value is entered again.
 */
export function launchChanged(
  form: ServerForm,
  listed: ListedServer | undefined,
): boolean {
  if (form.editing === undefined || listed === undefined) return false
  return !namesKept(form, listed) || form.env.some((row) => row.stored && row.edited)
}

/**
 * Whether a stored variable still waits for its value, the launch having
 * changed (U33, V2–V5): one not edited. An edited empty value is one entered (S7).
 */
export function valuesNeeded(
  form: ServerForm,
  listed: ListedServer | undefined,
): boolean {
  return launchChanged(form, listed) && form.env.some((row) => row.stored && !row.edited)
}

/**
 * Whether the form holds what a save needs before the gateway can judge it: a
 * non-blank name and command (U7), and every stored value again once the
 * launch changed (U33).
 */
export function formReady(form: ServerForm, listed: ListedServer | undefined): boolean {
  if (form.name.trim() === "") return false
  if (form.kind === "remote") return form.url.trim() !== ""
  return form.command.trim() !== "" && !valuesNeeded(form, listed)
}

/** The words for a remote row's authorization, when there is something to say. */
export function authorizationLabel(server: ListedServer): string | undefined {
  const auth = server.authorization
  if (!server.url || !auth) return undefined
  if (auth.phase === "revocation_incomplete" || auth.phase === "revoking")
    return sentences.revocationIncomplete
  if (auth.scopeRequired || auth.phase === "scope_required")
    return sentences.scopeRequired
  if (auth.tokenExpired) return sentences.tokenExpired
  if (auth.refreshFailing && auth.phase === "authorization_incomplete")
    return sentences.refreshFailing
  if (auth.phase === "pending_consent") return sentences.pendingConsent
  if (auth.phase === "consent_needed") return sentences.consentNeeded
  if (auth.phase === "authorization_incomplete") return sentences.authorizationIncomplete
  return undefined
}

/** The listed server the open form edits, when it edits one and the list still has it. */
export function editedServer(state: McpServersState): ListedServer | undefined {
  const editing = state.form?.editing
  return editing === undefined ? undefined : server(state, editing)
}

/** The servers the form edits, and the managed one, from a list. */
export function storedServers(list: ServerList) {
  return {
    stored: list.servers.filter((server) => !server.managed),
    managed: list.servers.filter((server) => server.managed),
  }
}

/** How many stored servers a list has under `name`. */
function countIn(list: ServerList, name: string): number {
  return storedServers(list).stored.filter((each) => each.name === name).length
}

/**
 * Whether more than one stored server is listed under `name` — a config
 * edited by hand. The gateway addresses a server by name, so none of them
 * can be edited, switched or inspected; a remove takes the first stored (G3).
 */
export function sharesName(state: McpServersState, name: string): boolean {
  const list = listed(state)
  return list !== undefined && countIn(list, name) > 1
}

/** A stored server as drawn: keyed by its occurrence id (G1). */
export interface ListedRow {
  readonly id: number
  readonly server: ListedServer
}

/** The stored servers under one name, at the first one's place (G2, G3). */
export interface ServerGroup {
  readonly name: string
  readonly rows: readonly ListedRow[]
}

/** A listed state's stored servers, grouped by name in stored order, and the managed ones. */
export function groupsOf(list: Extract<ListState, { phase: "listed" }>) {
  const groups: { name: string; rows: ListedRow[] }[] = []
  const managed: ListedRow[] = []
  list.list.servers.forEach((server, at) => {
    const row = { id: list.ids[at], server }
    if (server.managed) return managed.push(row)
    const group = groups.find((each) => each.name === server.name)
    if (group) group.rows.push(row)
    else groups.push({ name: server.name, rows: [row] })
  })
  return {
    groups: groups as readonly ServerGroup[],
    managed: managed as readonly ListedRow[],
  }
}

/* ——— A variable's value ——— */

const lineBreak = /\r\n|\r|\n/g
const trailingBreak = /(?:\r\n|\r|\n)$/

/** Whether a value holds a line break: a password field cannot (S4). */
export const hasLineBreak = (value: string) => /[\r\n]/.test(value)

/** How many line breaks a value holds, a CRLF one. */
export const lineBreaks = (value: string) => value.match(lineBreak)?.length ?? 0

/** Whether a value ends in a line break, as a key copied from a terminal may (S5). */
export const endsWithLineBreak = (value: string) => trailingBreak.test(value)

/** How many lines a held value is: its line breaks, but for one at its end, and one. */
export const linesOf = (value: string) =>
  lineBreaks(value) - (endsWithLineBreak(value) ? 1 : 0) + 1

/* ——— The requests ——— */

/**
 * The save a form asks for. A stored variable not edited keeps its value
 * (`null`); edited, its value is sent, the empty one too (S1, S2). A row with
 * neither name nor value is a blank one and is not sent.
 */
export function saveRequestOf(form: ServerForm, revision: string): SaveRequest {
  const renamed = form.editing !== undefined && form.editing !== form.name
  const previousName = renamed ? { previousName: form.editing } : {}
  if (form.kind === "remote") {
    return {
      revision,
      ...previousName,
      server: {
        kind: "remote",
        name: form.name,
        url: form.url,
        enabled: form.enabled,
      },
    }
  }
  return {
    revision,
    ...previousName,
    server: {
      kind: "stdio",
      name: form.name,
      command: form.command,
      args: argsOf(form),
      env: form.env.filter(sent).map((row) => ({
        name: row.name,
        value: row.stored && !row.edited ? null : row.value,
      })),
      enabled: form.enabled,
    },
  }
}

/** The save a switch sends: the server as listed, every value kept, the other way on. */
export function toggleRequestOf(server: ListedServer, revision: string): SaveRequest {
  if (server.url !== undefined) {
    return {
      revision,
      server: {
        kind: "remote",
        name: server.name,
        url: server.url,
        enabled: !server.enabled,
      },
    }
  }
  return {
    revision,
    server: {
      kind: "stdio",
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

const storedRow = (key: number, name: string): VariableRow => ({
  key,
  name,
  value: "",
  stored: true,
  edited: false,
  held: false,
})

/** Rows for what a list says, each with a key handed out after `rows`. */
function rowsOf(rows: number, args: readonly string[], envNames: readonly string[]) {
  return {
    args: args.map((value) => ({ key: ++rows, value })),
    env: envNames.map((variable) => storedRow(++rows, variable)),
    rows,
  }
}

function editForm(state: McpServersState, name: string): McpServersState {
  const found = server(state, name)
  if (!found || found.managed) return state
  const { args, env, rows } = rowsOf(state.rows, found.args, found.envNames)
  return {
    ...state,
    rows,
    notice: null,
    confirming: null,
    form: {
      editing: found.name,
      base: found,
      name: found.name,
      command: found.command,
      args,
      env,
      enabled: found.enabled,
      kind: found.url === undefined ? "stdio" : "remote",
      url: found.url ?? "",
    },
  }
}

/**
 * The open form against a list just read (U41, U42). A server no longer
 * listed closes the form, saying so. A server changed there has each field
 * still as the form was filled refilled from the list; a field typed here
 * and changed there too is kept as typed, and the notice names it, so the
 * other change is not overwritten unsaid.
 */
function refilled(state: McpServersState): McpServersState {
  const form = state.form
  if (!form || form.editing === undefined || !form.base) return state
  const now = server(state, form.editing)
  if (!now || now.managed)
    return {
      ...state,
      form: null,
      // A save not confirmed may itself have moved it (a rename): what it
      // said stands (F9).
      notice: form.unconfirmed
        ? state.notice
        : said(sentences.formGone(form.editing), "write"),
    }
  // Stored more than once since: no save could say which (G8).
  if (sharesName(state, form.editing))
    return {
      ...state,
      form: null,
      notice: said(sentences.formShared(form.editing), "write"),
    }
  const base = form.base
  const clashes: string[] = []
  let rows = state.rows
  let { command, args, enabled, env, url } = form
  if (form.kind === "remote" && now.url !== base.url) {
    if (form.url === (base.url ?? "")) url = now.url ?? ""
  }
  if (now.command !== base.command) {
    if (form.command === base.command) command = now.command
    else if (form.command !== now.command) clashes.push("the command")
  }
  if (!sameArgs(now.args, base.args)) {
    if (sameArgs(argsOf(form), base.args)) {
      args = now.args.map((value) => ({ key: ++rows, value }))
    } else if (!sameArgs(argsOf(form), now.args)) clashes.push("the arguments")
  }
  if (now.enabled !== base.enabled) {
    if (form.enabled === base.enabled) enabled = now.enabled
    else if (form.enabled !== now.enabled) clashes.push("whether it is offered")
  }
  if (!sameArgs(now.envNames, base.envNames)) {
    let typedGone = false
    env = form.env.flatMap((row) => {
      if (!row.stored || now.envNames.includes(row.name)) return [row]
      // Removed there: untouched, it goes; edited, it is a new one (S8).
      if (!row.edited) return []
      typedGone = true
      return [{ ...row, stored: false }]
    })
    const addedThere = now.envNames.filter((name) => !base.envNames.includes(name))
    // Added there and typed here too: the save would replace that value (F8).
    const typedBoth = addedThere.some((name) =>
      env.some((row) => !row.stored && row.name === name),
    )
    const added = addedThere
      .filter((name) => !env.some((row) => row.name === name))
      .map((name) => storedRow(++rows, name))
    env = [...env, ...added]
    if (typedGone || typedBoth) clashes.push("the variables")
  }
  const notice =
    clashes.length === 0
      ? state.notice
      : said(
          [state.notice?.text, sentences.changedThere(clashes)].filter(Boolean).join(" "),
          "write",
        )
  return {
    ...state,
    rows,
    notice,
    form: {
      ...form,
      base: now,
      command,
      args,
      enabled,
      env,
      url,
      unconfirmed: undefined,
    },
  }
}

/**
 * A confirm kept across a list read only while its name is stored as many
 * times as when it was asked: which server is first, and what the confirm
 * says, hold only then (G7).
 */
function stillAsked(confirming: Confirming | null, list: ServerList): Confirming | null {
  if (confirming === null || confirming.count === null) return confirming
  return countIn(list, confirming.name) === confirming.count ? confirming : null
}

/**
 * Each listed server's occurrence id (G1): the k-th server named N reuses the
 * id of the k-th one named N the previous list had, else gets a new one, so a
 * row keeps its key when the rows around it come and go.
 */
function idsFor(state: McpServersState, list: ServerList) {
  const before = state.list.phase === "listed" ? state.list : undefined
  const place = (server: ListedServer) => `${server.managed}:${server.name}`
  const previous = new Map<string, number[]>()
  before?.list.servers.forEach((server, at) => {
    const ids = previous.get(place(server)) ?? []
    previous.set(place(server), [...ids, before.ids[at]])
  })
  const seen = new Map<string, number>()
  let rows = state.rows
  const ids = list.servers.map((server) => {
    const k = seen.get(place(server)) ?? 0
    seen.set(place(server), k + 1)
    return previous.get(place(server))?.[k] ?? ++rows
  })
  return { ids, rows }
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
  if (outcome.ok) {
    const list = outcome.value as ServerList
    const { ids, rows } = idsFor(state, list)
    return refilled(
      inspectionOfStored({
        ...done,
        rows,
        // A list given is the gateway's yes.
        access: "admin",
        list: { phase: "listed", list, ids },
        confirming: stillAsked(state.confirming, list),
        // This list answers a list that failed; a write's notice stands.
        notice: state.notice?.from === "list" ? null : state.notice,
      }),
    )
  }
  const { failure } = outcome
  if (failure.kind === "forbidden") return forbidden(state)
  if (failure.kind === "notConfigured")
    return { ...done, list: { phase: "notConfigured" }, form: null, notice: null }
  if (failure.kind === "configTooLarge" && failure.revision !== undefined)
    // No server can be shown, so nothing is edited or confirmed against one;
    // a name typed before this list stays typed (U44, U46). Refused only
    // after the gateway let this credential ask, so it is a yes too.
    return {
      ...done,
      access: "admin",
      list: {
        phase: "tooLarge",
        revision: failure.revision,
        name: state.list.phase === "tooLarge" ? state.list.name : "",
      },
      form: null,
      confirming: null,
      notice: state.notice?.from === "list" ? null : state.notice,
    }
  return {
    ...done,
    list: { phase: "failed" },
    notice: said(listSentence(failure, state), "list"),
  }
}

/** The form, marked as one whose save's outcome is not known (F9). */
const unconfirmedIf = (form: ServerForm | null, unknown: boolean): ServerForm | null =>
  form && unknown ? { ...form, unconfirmed: true } : form

function authRequest(
  state: McpServersState,
  name: string,
  kind: "authorize" | "revoke",
): McpServersState {
  const list = listed(state)
  const found = server(state, name)
  if (!canWrite(state) || !list || !found?.remoteId) return state
  const seq = state.seq + 1
  return {
    ...state,
    seq,
    notice: null,
    pending: { kind, seq, id: found.remoteId, name, revision: list.revision },
  }
}

function answeredAuth(
  state: McpServersState,
  pending: Extract<PendingRequest, { kind: "authorize" | "revoke" }>,
  outcome: Outcome<unknown>,
): McpServersState {
  const done = { ...state, pending: null }
  if (!outcome.ok) {
    if (outcome.failure.kind === "forbidden") return forbidden(state)
    return {
      ...done,
      notice: said(failureText(outcome.failure), "write"),
    }
  }
  if (pending.kind === "authorize") {
    const value = outcome.value as AuthorizeResult
    if (value.status === "pending_consent" && value.consentUrl)
      return {
        ...done,
        consent: { name: pending.name, url: value.consentUrl },
        notice: said(sentences.pendingConsent, "write"),
      }
    return listAgain({
      ...done,
      consent: null,
      notice: said(
        value.status === "not_required"
          ? sentences.authorizeNotRequired
          : sentences.authorizeReady,
        "write",
      ),
    })
  }
  const value = outcome.value as RevokeResult
  return listAgain({
    ...done,
    consent: state.consent?.name === pending.name ? null : state.consent,
    notice: said(
      value.settled ? sentences.revokeSettled : sentences.revokeIncomplete,
      "write",
    ),
  })
}

function failureText(failure: Failure): string {
  switch (failure.kind) {
    case "authorizationHeld":
      return sentences.authorizationHeld
    case "storeUnavailable":
      return sentences.storeUnavailable
    case "registrationUnsupported":
      return sentences.registrationUnsupported
    case "discoveryFailed":
      return sentences.discoveryFailed
    case "authorizationIncomplete":
      return sentences.authorizationIncomplete
    case "busy":
      return sentences.busy
    case "notFound":
      return sentences.notFound
    case "revisionConflict":
      return sentences.conflict
    default:
      return sentences.unanswered
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
      // Stored, but not what new conversations get yet: said (L1–L5).
      notice: (outcome.value as WriteResult).live
        ? null
        : said(
            pending.kind === "remove"
              ? sentences.notLiveRemoved
              : sentences.notLiveSaved(fromForm ? "save" : "change"),
            "write",
          ),
      form: fromForm ? null : state.form,
      confirming: pending.kind === "remove" ? null : state.confirming,
      // The name removed is not typed again for the list read next (U45).
      list: state.list.phase === "tooLarge" ? { ...state.list, name: "" } : state.list,
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
            form: {
              ...state.form,
              // A value missing because the launch changed says why (U35).
              problem:
                failure.problem === "environmentValueMissing" &&
                launchChanged(state.form, editedServer(state))
                  ? { field: "env", text: sentences.valuesAgain }
                  : problemAt(failure),
            },
          }
        : { ...done, notice: said(writeSentence(failure, state), "write") }
    case "configTooLarge":
      // Nothing was written (W1), so nothing is listed again: the form stays
      // open with what was typed, to make smaller (U48); a switch says so (U49).
      return fromForm && state.form
        ? {
            ...done,
            notice: null,
            form: {
              ...state.form,
              problem: { field: "form", text: sentences.saveTooLarge },
            },
          }
        : {
            ...done,
            notice: said(
              pending.kind === "save"
                ? sentences.saveTooLarge
                : writeSentence(failure, state),
              "write",
            ),
          }
    case "notFound":
      // A name typed for a list too large to show was not one stored (U46).
      return listAgain({
        ...done,
        notice: said(
          state.list.phase === "tooLarge" && pending.kind === "remove"
            ? sentences.noSuchServer(pending.request.name)
            : sentences.notFound,
          "write",
        ),
        confirming: null,
      })
    case "busy":
    case "stopping":
    case "reservedName":
      // Nothing was written; the controls come back as they were.
      return { ...done, notice: said(writeSentence(failure, state), "write") }
    case "auditUnavailable":
      return listAgain({
        ...done,
        notice: said(writeSentence(failure, state), "write"),
        form:
          failure.applied === true && fromForm
            ? null
            : unconfirmedIf(state.form, fromForm && failure.applied === undefined),
        confirming: null,
      })
    case "authorizationHeld":
      // The file was written. The live set was not replaced, so the form
      // closes and the list is read; the notice says the old authorization
      // still holds the previous address.
      return listAgain({
        ...done,
        notice: said(sentences.authorizationHeld, "write"),
        form: fromForm ? null : state.form,
        confirming: null,
      })
    case "storageUnavailable":
      // Published but not synced is a change made: said as one, the form
      // closed, as a write that was recorded would be (U36).
      return listAgain({
        ...done,
        notice: said(
          failure.applied === true
            ? sentences.notDurable(pending.kind)
            : writeSentence(failure, state),
          "write",
        ),
        form:
          failure.applied === true && fromForm
            ? null
            : unconfirmedIf(state.form, fromForm && failure.applied === undefined),
        confirming: null,
      })
    default:
      // The list shows where things stand: what was typed is kept to try again.
      // A conflict or a file unreadable wrote nothing; the rest may have.
      return listAgain({
        ...done,
        notice: said(writeSentence(failure, state), "write"),
        form: unconfirmedIf(
          state.form,
          fromForm &&
            failure.kind !== "revisionConflict" &&
            failure.kind !== "configInvalid",
        ),
        confirming: null,
      })
  }
}

/** The form with its variable keyed `key` changed, while nothing is in flight. */
function withVariable(
  state: McpServersState,
  key: number,
  change: (row: VariableRow) => VariableRow,
): McpServersState {
  if (!state.form || state.pending) return state
  return {
    ...state,
    form: {
      ...state.form,
      env: state.form.env.map((row) => (row.key === key ? change(row) : row)),
    },
  }
}

export function mcpServersReducer(
  state: McpServersState,
  event: McpServersEvent,
): McpServersState {
  switch (event.type) {
    case "connected": {
      const next: McpServersState = { ...state, connection: "connected" }
      // Listed again on every connection: what changed while away is read,
      // and the gateway's answer says whether this credential may manage.
      return next.pending === null ? listAgain(next) : next
    }
    case "unreachable":
      return { ...state, connection: "unreachable" }
    case "answered": {
      const pending = state.pending
      if (pending === null || pending.seq !== event.seq) return state
      if (pending.kind === "authorize" || pending.kind === "revoke")
        return answeredAuth(state, pending, event.outcome)
      return pending.kind === "list"
        ? answeredList(state, event.outcome)
        : answeredWrite(state, pending, event.outcome)
    }
    case "retry":
      if (
        state.access === "notAdmin" ||
        state.connection !== "connected" ||
        state.pending
      )
        return state
      return listAgain({ ...state, notice: null })
    case "add":
      if (!canWrite(state)) return state
      return {
        ...state,
        notice: null,
        confirming: null,
        form: {
          name: "",
          command: "",
          args: [],
          env: [],
          enabled: true,
          kind: "stdio",
          url: "",
        },
      }
    case "edit":
      return canWrite(state) && !sharesName(state, event.name)
        ? editForm(state, event.name)
        : state
    case "change":
      if (!state.form || state.pending) return state
      return { ...state, form: { ...state.form, ...event.patch } }
    case "addArgument": {
      if (!state.form || state.pending) return state
      const row = { key: state.rows + 1, value: "" }
      const after = state.form.args.findIndex((each) => each.key === event.after)
      const args =
        after === -1
          ? [...state.form.args, row]
          : [
              ...state.form.args.slice(0, after + 1),
              row,
              ...state.form.args.slice(after + 1),
            ]
      return { ...state, rows: row.key, form: { ...state.form, args } }
    }
    case "changeArgument":
      if (!state.form || state.pending) return state
      return {
        ...state,
        form: {
          ...state.form,
          args: state.form.args.map((row) =>
            row.key === event.key ? { ...row, value: event.value } : row,
          ),
        },
      }
    case "removeArgument":
      if (!state.form || state.pending) return state
      return {
        ...state,
        form: {
          ...state.form,
          args: state.form.args.filter((row) => row.key !== event.key),
        },
      }
    case "addVariable":
      if (!state.form || state.pending) return state
      return {
        ...state,
        rows: state.rows + 1,
        form: {
          ...state.form,
          env: [
            ...state.form.env,
            {
              key: state.rows + 1,
              name: "",
              value: "",
              stored: false,
              edited: false,
              held: false,
            },
          ],
        },
      }
    case "changeVariable": {
      const { name, value } = event.patch
      return withVariable(state, event.key, (row) => ({
        ...row,
        // A stored variable's name is the one stored; only its value changes.
        ...(name === undefined || row.stored ? {} : { name }),
        // Typed: what the field holds, drawn (S3).
        ...(value === undefined ? {} : { value, edited: true, held: false }),
      }))
    }
    case "pasteVariable":
      return withVariable(state, event.key, (row) => ({
        ...row,
        value: event.value,
        edited: true,
        held: true,
      }))
    case "trimVariable":
      return withVariable(state, event.key, (row) =>
        row.held && endsWithLineBreak(row.value)
          ? { ...row, value: row.value.replace(trailingBreak, "") }
          : row,
      )
    case "clearVariable":
      return withVariable(state, event.key, (row) => ({
        ...row,
        value: "",
        edited: true,
        held: false,
      }))
    case "keepVariable":
      return withVariable(state, event.key, (row) =>
        row.stored ? { ...row, value: "", edited: false, held: false } : row,
      )
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
      if (
        !state.form ||
        !list ||
        !canWrite(state) ||
        !formReady(state.form, editedServer(state))
      )
        return state
      const seq = state.seq + 1
      const form = { ...state.form, problem: undefined, unconfirmed: undefined }
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
      if (
        !list ||
        !found ||
        found.managed ||
        sharesName(state, found.name) ||
        !canWrite(state)
      )
        return state
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
      const list = listed(state)
      const count = list ? countIn(list, event.name) : 0
      if (count === 0 || !canWrite(state)) return state
      return { ...state, confirming: { name: event.name, count }, notice: null }
    }
    case "cancelRemove":
      if (state.pending) return state
      return { ...state, confirming: null }
    case "changeRemoveName":
      if (state.list.phase !== "tooLarge" || state.pending) return state
      return {
        ...state,
        confirming: null,
        list: { ...state.list, name: event.name },
      }
    case "askRemoveByName":
      if (state.list.phase !== "tooLarge" || !canRemoveByName(state)) return state
      return {
        ...state,
        confirming: { name: state.list.name, count: null },
        notice: null,
      }
    case "confirmRemove": {
      // At the list's revision, or the one the too-large refusal named (U45).
      const revision =
        state.list.phase === "tooLarge" ? state.list.revision : listed(state)?.revision
      const may =
        state.list.phase === "tooLarge" ? canRemoveByName(state) : canWrite(state)
      if (revision === undefined || state.confirming === null || !may) return state
      const seq = state.seq + 1
      return {
        ...state,
        seq,
        pending: {
          kind: "remove",
          seq,
          request: { revision, name: state.confirming.name },
        },
      }
    }
    case "authorize":
      return authRequest(state, event.name, "authorize")
    case "revoke":
      return authRequest(state, event.name, "revoke")
    case "inspect": {
      const found = server(state, event.name)
      // Shared, the gateway would start the first: not the one asked about (G4).
      if (!found || found.managed || sharesName(state, found.name) || !canInspect(state))
        return state
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
