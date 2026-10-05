import {
  createContext,
  useContext,
  useEffect,
  useId,
  useReducer,
  useRef,
  type KeyboardEvent,
  type ReactNode,
} from "react"
import type { McpServersGateway } from "../adapters/mcp-servers-gateway"
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
  storedServers,
  type FormField,
  type InspectedTool,
  type ListedServer,
  type McpServersEvent,
  type McpServersState,
  type ServerForm,
} from "../model/mcp-servers"
import { DesktopIcon } from "../../ui/icons"
import { PendingAction, SettingGroup, Toggle } from "./settings-controls"

/**
 * Settings › Connections › Integrations: the gateway's stored MCP servers
 * (#391). The rules are the reducer's (`model/mcp-servers.ts`); this draws
 * its state and sends the one request it names. With no gateway — the
 * sample preview, whose workspace is the in-memory one — the page keeps its
 * pending row.
 *
 * Focus follows what opens and closes (CHECKLIST › Integrations): Add or
 * Edit puts it on the form's first field, Remove on the confirm's Cancel,
 * Inspect on the panel's heading; closing any of them puts it back on the
 * row's button that opened it, or on Add (`focusAfter`). Escape closes the
 * form and the confirm.
 */

/** The stored servers of the window's gateway, given by composition (`main.tsx`); none without one. */
const McpServersContext = createContext<McpServersGateway | undefined>(undefined)

export function McpServersProvider({
  gateway,
  children,
}: {
  gateway: McpServersGateway | undefined
  children: ReactNode
}) {
  return (
    <McpServersContext.Provider value={gateway}>{children}</McpServersContext.Provider>
  )
}

const footnote = "Servers added here are offered to every agent that supports MCP."

export function IntegrationsTab() {
  const gateway = useContext(McpServersContext)
  if (!gateway)
    return (
      <SettingGroup id="mcp-servers" footnote={footnote} pending>
        <div className="settings-empty">
          <DesktopIcon name="connections" />
          <p>{sentences.empty}</p>
          <PendingAction>Add server…</PendingAction>
        </div>
      </SettingGroup>
    )
  return <ManagedServers gateway={gateway} />
}

type Dispatch = (event: McpServersEvent) => void

/** Sends the request the state names, once for each `seq`, and hands its answer back. */
function useRequests(
  gateway: McpServersGateway,
  state: McpServersState,
  dispatch: Dispatch,
) {
  const sent = useRef(0)
  const inspected = useRef(0)
  const pending = state.pending
  useEffect(() => {
    // Once per request, even where the effect runs twice for one (StrictMode).
    if (!pending || sent.current === pending.seq) return
    sent.current = pending.seq
    const answer =
      pending.kind === "list"
        ? gateway.list()
        : pending.kind === "save"
          ? gateway.save(pending.request)
          : gateway.remove(pending.request)
    void answer.then((outcome) =>
      dispatch({ type: "answered", seq: pending.seq, outcome }),
    )
  }, [gateway, pending, dispatch])
  const running = state.inspection?.phase === "running" ? state.inspection : undefined
  useEffect(() => {
    if (!running || inspected.current === running.seq) return
    inspected.current = running.seq
    void gateway
      .inspect(running.name)
      .then((outcome) => dispatch({ type: "inspected", seq: running.seq, outcome }))
  }, [gateway, running, dispatch])
}

/** A button focus may go back to: a row's, by its server's name, or Add. */
export interface FocusTarget {
  readonly action: "edit" | "inspect" | "remove" | "add" | "removeByName"
  readonly server?: string
}

/**
 * Where focus goes back to when the form, the confirm or the inspection
 * closes between `previous` and `next`: the first of these drawn, once it is
 * enabled. `null` when nothing closed. A saved form goes back to the row
 * under the name saved, a cancelled one to the row it edited; a removed row
 * is gone, so Add.
 */
export function focusAfter(
  previous: McpServersState,
  next: McpServersState,
): readonly FocusTarget[] | null {
  const add: FocusTarget = { action: "add" }
  if (previous.form && !next.form)
    return previous.form.editing === undefined
      ? [add]
      : [
          { action: "edit", server: previous.form.name },
          { action: "edit", server: previous.form.editing },
          add,
        ]
  if (previous.confirming !== null && next.confirming === null)
    return [
      { action: "remove", server: previous.confirming },
      // A list too large to show has no rows: back to the name field (U44).
      { action: "removeByName" },
      add,
    ]
  if (previous.inspection && !next.inspection)
    return [{ action: "inspect", server: previous.inspection.name }, add]
  return null
}

function buttonFor(container: HTMLElement, target: FocusTarget) {
  const scope =
    target.server === undefined
      ? container
      : [...container.querySelectorAll("[data-mcp-server]")].find(
          (row) => row.getAttribute("data-mcp-server") === target.server,
        )
  return scope?.querySelector<HTMLButtonElement | HTMLInputElement>(
    `[data-mcp-action="${target.action}"]`,
  )
}

/**
 * Puts focus back where `focusAfter` says, once the button is there and
 * enabled — a save's is only after the list it reads. Only focus that was
 * lost (on the body, or a surface around the tab) or is still in the tab is
 * moved: focus taken elsewhere meanwhile stays there.
 */
function useFocusReturn(state: McpServersState) {
  const container = useRef<HTMLDivElement>(null)
  const previous = useRef(state)
  const returning = useRef<readonly FocusTarget[] | null>(null)
  useEffect(() => {
    const after = focusAfter(previous.current, state)
    previous.current = state
    if (after) returning.current = after
    const targets = returning.current
    const tab = container.current
    if (!targets || !tab) return
    // Lost focus is on the body, or — where a click does not focus a button,
    // as in WebKit — on the nearest focusable thing around the tab.
    const active = document.activeElement
    if (active && !tab.contains(active) && !active.contains(tab)) {
      returning.current = null
      return
    }
    for (const target of targets) {
      const button = buttonFor(tab, target)
      if (!button) continue
      // Drawn but resting while a request runs: wait for it.
      if (button.disabled) return
      button.focus()
      break
    }
    returning.current = null
  })
  return container
}

function ManagedServers({ gateway }: { gateway: McpServersGateway }) {
  const [state, dispatch] = useReducer(
    mcpServersReducer,
    gateway.limits,
    initialMcpServersState,
  )
  useEffect(() => gateway.follow(dispatch), [gateway])
  useRequests(gateway, state, dispatch)
  const container = useFocusReturn(state)
  const phase =
    state.access === "notAdmin"
      ? "not-admin"
      : state.list.phase === "listed"
        ? "listed"
        : state.list.phase === "notConfigured"
          ? "not-configured"
          : state.list.phase === "failed"
            ? "failed"
            : state.list.phase === "tooLarge"
              ? "too-large"
              : "loading"
  return (
    <div
      ref={container}
      className="settings-servers"
      data-mcp-servers={phase}
      data-connection={state.connection}
      aria-busy={state.pending !== null || undefined}
    >
      <SettingGroup id="mcp-servers" footnote={footnote}>
        <ServersCard state={state} dispatch={dispatch} />
      </SettingGroup>
      {state.form && state.access === "admin" ? (
        <FormGroup state={state} form={state.form} dispatch={dispatch} />
      ) : null}
      {state.inspection && state.access === "admin" ? (
        <InspectionGroup state={state} dispatch={dispatch} />
      ) : null}
    </div>
  )
}

/**
 * What the gateway's answers say: one live region, there from the tab's
 * first draw so what arrives in it later is read out.
 */
function Notices({ state }: { state: McpServersState }) {
  return (
    <div className="settings-notices" role="status" data-mcp-notices>
      {state.connection === "unreachable" ? (
        <p className="settings-notice" data-mcp-unreachable>
          {sentences.unreachable}
        </p>
      ) : null}
      {state.notice ? (
        <p className="settings-notice" data-mcp-notice>
          {state.notice.text}
        </p>
      ) : null}
    </div>
  )
}

function ServersCard({
  state,
  dispatch,
}: {
  state: McpServersState
  dispatch: Dispatch
}) {
  if (state.access === "notAdmin")
    return (
      <div className="settings-empty">
        <DesktopIcon name="privacy" />
        <p>{sentences.notAdmin}</p>
      </div>
    )
  return (
    <>
      <Notices state={state} />
      <ServersBody state={state} dispatch={dispatch} />
    </>
  )
}

function ServersBody({
  state,
  dispatch,
}: {
  state: McpServersState
  dispatch: Dispatch
}) {
  const list = state.list
  if (list.phase === "notConfigured")
    return (
      <>
        <div className="settings-empty">
          <DesktopIcon name="connections" />
          <p>{sentences.notConfigured}</p>
        </div>
      </>
    )
  if (list.phase === "failed")
    return (
      <>
        <div className="settings-empty">
          <button
            type="button"
            className="settings-button"
            disabled={state.connection !== "connected" || state.pending !== null}
            onClick={() => dispatch({ type: "retry" })}
          >
            Try Again
          </button>
        </div>
      </>
    )
  if (list.phase === "tooLarge")
    return <TooLargeBody state={state} list={list} dispatch={dispatch} />
  const writable = canWrite(state) && state.form === null
  const add = (
    <button
      type="button"
      className="settings-button"
      data-mcp-action="add"
      disabled={!writable}
      onClick={() => dispatch({ type: "add" })}
    >
      Add server…
    </button>
  )
  if (list.phase === "loading")
    return (
      <>
        <div
          className="settings-row settings-skeleton"
          data-mcp-skeleton
          aria-hidden="true"
        >
          <span />
        </div>
        <div
          className="settings-row settings-skeleton"
          data-mcp-skeleton
          aria-hidden="true"
        >
          <span />
        </div>
        <div className="settings-row settings-servers-add">{add}</div>
      </>
    )
  const { stored, managed } = storedServers(list.list)
  return (
    <>
      {stored.length === 0 ? (
        <div className="settings-empty" data-mcp-empty>
          <DesktopIcon name="connections" />
          <p>{sentences.empty}</p>
          {add}
        </div>
      ) : (
        <>
          {/* Keyed by where each is listed: a name stored twice by hand is two rows. */}
          {stored.map((server, at) => (
            <ServerRow
              key={`${at}:${server.name}`}
              server={server}
              state={state}
              dispatch={dispatch}
            />
          ))}
          <div className="settings-row settings-servers-add">{add}</div>
        </>
      )}
      {managed.map((server, at) => (
        <ServerRow
          key={`managed:${at}:${server.name}`}
          server={server}
          state={state}
          dispatch={dispatch}
        />
      ))}
    </>
  )
}

/**
 * A list too large to show (U44): the refusal names no server, so none is
 * drawn. A server is removed by the name typed, asked about as a row's
 * Remove is, at the refusal's revision.
 */
function TooLargeBody({
  state,
  list,
  dispatch,
}: {
  state: McpServersState
  list: Extract<McpServersState["list"], { phase: "tooLarge" }>
  dispatch: Dispatch
}) {
  const fieldId = useId()
  const textId = useId()
  const askId = useId()
  const cancel = useRef<HTMLButtonElement>(null)
  const confirming = state.confirming !== null
  const resting = state.connection !== "connected" || state.pending !== null
  useEffect(() => {
    if (confirming) cancel.current?.focus()
  }, [confirming])
  const onKeyDown = (event: KeyboardEvent) => {
    if (!confirming || event.key !== "Escape" || state.pending !== null) return
    event.preventDefault()
    event.stopPropagation()
    dispatch({ type: "cancelRemove" })
  }
  return (
    <div
      className="settings-row settings-server"
      data-mcp-too-large
      onKeyDown={onKeyDown}
    >
      <div className="settings-row-text">
        <p id={textId}>{sentences.listTooLarge}</p>
        <label htmlFor={fieldId}>Server name</label>
        <input
          id={fieldId}
          className="settings-input"
          data-mcp-action="removeByName"
          value={list.name}
          autoComplete="off"
          spellCheck={false}
          aria-describedby={textId}
          disabled={resting}
          onChange={(event) =>
            dispatch({ type: "changeRemoveName", name: event.target.value })
          }
        />
        {state.confirming !== null ? (
          <p id={askId} className="settings-server-confirm" data-mcp-confirm>
            {sentences.removeAsk(state.confirming)}
          </p>
        ) : null}
      </div>
      <div className="settings-row-control settings-server-actions">
        {confirming
          ? [
              <button
                key="cancel"
                ref={cancel}
                type="button"
                className="settings-button"
                data-mcp-action="cancel"
                aria-describedby={askId}
                disabled={state.pending !== null}
                onClick={() => dispatch({ type: "cancelRemove" })}
              >
                Cancel
              </button>,
              <button
                key="confirm"
                type="button"
                className="settings-button settings-button-danger"
                data-mcp-action="confirm"
                aria-describedby={askId}
                disabled={!canRemoveByName(state)}
                onClick={() => dispatch({ type: "confirmRemove" })}
              >
                Remove
              </button>,
            ]
          : [
              <button
                key="remove"
                type="button"
                className="settings-button"
                data-mcp-action="remove"
                disabled={!canRemoveByName(state)}
                onClick={() => dispatch({ type: "askRemoveByName" })}
              >
                Remove
              </button>,
            ]}
      </div>
    </div>
  )
}

function ServerRow({
  server,
  state,
  dispatch,
}: {
  server: ListedServer
  state: McpServersState
  dispatch: Dispatch
}) {
  const nameId = useId()
  const askId = useId()
  const cancel = useRef<HTMLButtonElement>(null)
  const writable = canWrite(state) && state.form === null
  const confirming = state.confirming === server.name
  const command = [server.command, ...server.args].join(" ")
  useEffect(() => {
    if (confirming) cancel.current?.focus()
  }, [confirming])
  const onKeyDown = (event: KeyboardEvent) => {
    if (!confirming || event.key !== "Escape" || state.pending !== null) return
    // The confirm's Escape: Settings' own never closes it.
    event.preventDefault()
    event.stopPropagation()
    dispatch({ type: "cancelRemove" })
  }
  return (
    <div
      className="settings-row settings-server"
      data-mcp-server={server.name}
      data-managed={server.managed || undefined}
      onKeyDown={onKeyDown}
    >
      <div className="settings-row-text">
        <span id={nameId}>{server.name}</span>
        <code className="settings-server-command">{command}</code>
        <small>
          {server.managed
            ? sentences.managed
            : sentences.variables(server.envNames.length)}
        </small>
        {confirming ? (
          <p id={askId} className="settings-server-confirm" data-mcp-confirm>
            {sentences.removeAsk(server.name)}
          </p>
        ) : null}
      </div>
      <div className="settings-row-control settings-server-actions">
        {/* Each button keyed: the confirm's are not the row's, reused by place. */}
        {server.managed
          ? null
          : confirming
            ? [
                <button
                  key="cancel"
                  ref={cancel}
                  type="button"
                  className="settings-button"
                  data-mcp-action="cancel"
                  aria-describedby={askId}
                  disabled={state.pending !== null}
                  onClick={() => dispatch({ type: "cancelRemove" })}
                >
                  Cancel
                </button>,
                <button
                  key="confirm"
                  type="button"
                  className="settings-button settings-button-danger"
                  data-mcp-action="confirm"
                  aria-describedby={askId}
                  disabled={!canWrite(state)}
                  onClick={() => dispatch({ type: "confirmRemove" })}
                >
                  Remove
                </button>,
              ]
            : [
                <button
                  key="edit"
                  type="button"
                  className="settings-button"
                  data-mcp-action="edit"
                  aria-describedby={nameId}
                  disabled={!writable}
                  onClick={() => dispatch({ type: "edit", name: server.name })}
                >
                  Edit
                </button>,
                <button
                  key="inspect"
                  type="button"
                  className="settings-button"
                  data-mcp-action="inspect"
                  aria-describedby={nameId}
                  disabled={!canInspect(state)}
                  onClick={() => dispatch({ type: "inspect", name: server.name })}
                >
                  Inspect
                </button>,
                <button
                  key="remove"
                  type="button"
                  className="settings-button"
                  data-mcp-action="remove"
                  aria-describedby={nameId}
                  disabled={!writable}
                  onClick={() => dispatch({ type: "askRemove", name: server.name })}
                >
                  Remove
                </button>,
              ]}
        <Toggle
          checked={server.enabled}
          label={server.name}
          disabled={server.managed || !writable || confirming}
          onChange={() => dispatch({ type: "toggle", name: server.name })}
        />
      </div>
    </div>
  )
}

/**
 * A field's problem, there from the form's first draw so a refusal arriving
 * in it is read out, and named by its field's `aria-describedby`.
 */
function FieldProblem({
  form,
  field,
  id,
}: {
  form: ServerForm
  field: FormField
  id: string
}) {
  return (
    <p id={id} className="settings-field-problem" role="alert" data-mcp-problem={field}>
      {form.problem?.field === field ? form.problem.text : null}
    </p>
  )
}

function FormGroup({
  state,
  form,
  dispatch,
}: {
  state: McpServersState
  form: ServerForm
  dispatch: Dispatch
}) {
  const ids = { name: useId(), command: useId(), args: useId(), values: useId() }
  const problems = {
    form: useId(),
    name: useId(),
    command: useId(),
    args: useId(),
    env: useId(),
  } satisfies Record<FormField, string>
  const first = useRef<HTMLInputElement>(null)
  const busy = state.pending !== null
  useEffect(() => {
    first.current?.focus()
  }, [])
  const change = (patch: Extract<McpServersEvent, { type: "change" }>["patch"]) =>
    dispatch({ type: "change", patch })
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || busy) return
    // The form's Escape: Settings' own never closes it.
    event.preventDefault()
    event.stopPropagation()
    dispatch({ type: "cancelForm" })
  }
  const described = (field: FormField) =>
    form.problem?.field === field ? problems[field] : undefined
  const listed = editedServer(state)
  const relaunched = launchChanged(form, listed)
  const waiting = valuesNeeded(form, listed)
  return (
    <section
      className="settings-group"
      data-mcp-form={form.editing ?? ""}
      onKeyDown={onKeyDown}
    >
      <h2>{form.editing === undefined ? "Add server" : `Edit “${form.editing}”`}</h2>
      <fieldset
        className="settings-card settings-form"
        disabled={busy}
        aria-describedby={described("form")}
      >
        <FieldProblem form={form} field="form" id={problems.form} />
        <div className="settings-field">
          <label htmlFor={ids.name}>Name</label>
          <input
            ref={first}
            id={ids.name}
            className="settings-input"
            value={form.name}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={form.problem?.field === "name" || undefined}
            aria-describedby={described("name")}
            onChange={(event) => change({ name: event.target.value })}
          />
          <FieldProblem form={form} field="name" id={problems.name} />
        </div>
        <div className="settings-field">
          <label htmlFor={ids.command}>Command</label>
          {/* A path is often wider than a narrow page: wrapped where it
              is read, as the row's command is, rather than cut off. */}
          <textarea
            id={ids.command}
            className="settings-input settings-input-mono settings-input-wrapped"
            data-mcp-field="command"
            rows={1}
            value={form.command}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={form.problem?.field === "command" || undefined}
            aria-describedby={described("command")}
            onChange={(event) => change({ command: event.target.value })}
          />
          <FieldProblem form={form} field="command" id={problems.command} />
        </div>
        <div className="settings-field" role="group" aria-labelledby={ids.args}>
          <span id={ids.args} className="settings-field-label">
            Arguments
          </span>
          {/* One field each, so an empty argument, or one with a line break,
              is one argument as typed. */}
          {form.args.map((row, at) => (
            <div className="settings-argument" key={row.key} data-mcp-argument={at}>
              <textarea
                className="settings-input settings-input-mono settings-input-wrapped"
                rows={1}
                aria-label={`Argument ${at + 1}`}
                value={row.value}
                autoComplete="off"
                spellCheck={false}
                aria-invalid={form.problem?.field === "args" || undefined}
                aria-describedby={described("args")}
                onChange={(event) =>
                  dispatch({
                    type: "changeArgument",
                    key: row.key,
                    value: event.target.value,
                  })
                }
              />
              <button
                type="button"
                className="settings-button"
                aria-label={`Remove argument ${at + 1}`}
                onClick={() => dispatch({ type: "removeArgument", key: row.key })}
              >
                Remove
              </button>
            </div>
          ))}
          <div>
            <button
              type="button"
              className="settings-button"
              data-mcp-action="add-argument"
              onClick={() => dispatch({ type: "addArgument" })}
            >
              Add argument
            </button>
          </div>
          <FieldProblem form={form} field="args" id={problems.args} />
        </div>
        <div className="settings-field">
          <span className="settings-field-label">Variables</span>
          {form.env.map((row) => (
            <div className="settings-variable" key={row.key} data-mcp-variable={row.name}>
              <input
                className="settings-input settings-input-mono"
                aria-label="Variable name"
                value={row.name}
                readOnly={row.stored}
                aria-describedby={described("env")}
                autoComplete="off"
                spellCheck={false}
                onChange={(event) =>
                  dispatch({
                    type: "changeVariable",
                    key: row.key,
                    patch: { name: event.target.value },
                  })
                }
              />
              {/* Uncontrolled, with no value or default given: React writes a
                  controlled field's value, and a default, into the markup as its
                  value attribute; this field's is only ever its own (U31).
                  Multiline and masked, so a pasted key keeps its line breaks. */}
              <textarea
                className="settings-input settings-input-mono settings-input-secret"
                rows={1}
                aria-label={`Value of ${row.name || "the variable"}`}
                aria-describedby={
                  [described("env"), row.stored && waiting ? ids.values : undefined]
                    .filter(Boolean)
                    .join(" ") || undefined
                }
                placeholder={
                  row.stored
                    ? relaunched
                      ? sentences.storedValueAgain
                      : sentences.storedValue
                    : "Value"
                }
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                data-mcp-secret
                onChange={(event) =>
                  dispatch({
                    type: "changeVariable",
                    key: row.key,
                    patch: { value: event.target.value },
                  })
                }
              />
              <button
                type="button"
                className="settings-button"
                aria-label={`Remove ${row.name || "this variable"}`}
                onClick={() => dispatch({ type: "removeVariable", key: row.key })}
              >
                Remove
              </button>
            </div>
          ))}
          <div>
            <button
              type="button"
              className="settings-button"
              onClick={() => dispatch({ type: "addVariable" })}
            >
              Add variable
            </button>
          </div>
          <p
            id={ids.values}
            className="settings-field-note"
            aria-live="polite"
            data-mcp-values-needed
          >
            {waiting ? sentences.valuesAgain : null}
          </p>
          <FieldProblem form={form} field="env" id={problems.env} />
        </div>
        <div className="settings-field settings-field-inline">
          <span className="settings-field-label">Offered to new conversations</span>
          <Toggle
            checked={form.enabled}
            label="Offered to new conversations"
            disabled={busy}
            onChange={(enabled) => change({ enabled })}
          />
        </div>
        <div className="settings-form-actions">
          <button
            type="button"
            className="settings-button"
            data-mcp-action="cancel"
            onClick={() => dispatch({ type: "cancelForm" })}
          >
            Cancel
          </button>
          <button
            type="button"
            className="settings-button settings-button-primary"
            disabled={!formReady(form, listed) || !canWrite(state)}
            onClick={() => dispatch({ type: "save" })}
          >
            Save
          </button>
        </div>
      </fieldset>
    </section>
  )
}

/** A hint's badge, shown only when the server gave the hint as a boolean. */
function hints(tool: InspectedTool): string[] {
  return [
    ...(tool.readOnly === true ? ["read-only"] : []),
    ...(tool.destructive === true ? ["destructive"] : []),
  ]
}

function InspectionGroup({
  state,
  dispatch,
}: {
  state: McpServersState
  dispatch: Dispatch
}) {
  const inspection = state.inspection
  const heading = useRef<HTMLHeadingElement>(null)
  const started = inspection?.phase === "running" ? inspection.seq : undefined
  useEffect(() => {
    if (started !== undefined) heading.current?.focus()
  }, [started])
  if (!inspection) return null
  // How it stands, in one live region drawn with the panel: what it starts,
  // then what it found or why it failed, read out as it arrives.
  const status =
    inspection.phase === "running"
      ? sentences.inspecting(inspection.name, state.limits.inspectDeadlineMs)
      : inspection.phase === "failed"
        ? inspection.text
        : inspection.result.cut === "stopping"
          ? sentences.cut.stopping
          : sentences.tools(inspection.result.tools.length)
  // A stopping cut is the status itself: it read no tools, so it is not a note under them.
  const cutNote =
    inspection.phase === "done" && inspection.result.cut !== "stopping"
      ? inspection.result.cut
      : undefined
  return (
    <section className="settings-group" data-mcp-inspection={inspection.phase}>
      <h2 ref={heading} tabIndex={-1}>
        Inspect “{inspection.name}”
      </h2>
      <div className="settings-card settings-inspection">
        <p
          className="settings-inspection-note"
          role="status"
          data-mcp-inspection-status
          data-mcp-inspection-failure={inspection.phase === "failed" || undefined}
        >
          {status}
        </p>
        {inspection.phase === "done" ? (
          <>
            {inspection.result.tools.length === 0 ? null : (
              <ul className="settings-tools">
                {/* By place too: a server may list one name twice. */}
                {inspection.result.tools.map((tool, at) => (
                  <li key={`${at}:${tool.name}`} data-mcp-tool={tool.name}>
                    <div className="settings-tool-head">
                      <code>{tool.name}</code>
                      {hints(tool).map((hint) => (
                        <span key={hint} className="settings-badge" data-badge={hint}>
                          {hint}
                        </span>
                      ))}
                      {tool.ui ? (
                        <span className="settings-badge" data-badge="ui">
                          UI
                        </span>
                      ) : null}
                    </div>
                    {tool.ui ? (
                      <dl className="settings-tool-ui">
                        <div>
                          <dt>App</dt>
                          <dd>
                            <code>{tool.ui.uri}</code>
                          </dd>
                        </div>
                        {tool.ui.csp.map((list) => (
                          <div key={list.name}>
                            <dt>{list.name}</dt>
                            <dd>{list.origins.join(", ")}</dd>
                          </div>
                        ))}
                        <div>
                          <dt>Permissions</dt>
                          <dd>
                            {tool.ui.permissions.length === 0
                              ? "None"
                              : tool.ui.permissions.join(", ")}
                          </dd>
                        </div>
                      </dl>
                    ) : null}
                  </li>
                ))}
              </ul>
            )}
            {cutNote ? (
              <p className="settings-inspection-note" data-mcp-cut={cutNote}>
                {sentences.cut[cutNote]}
              </p>
            ) : null}
          </>
        ) : null}
        <div className="settings-form-actions">
          <button
            type="button"
            className="settings-button"
            data-mcp-action="close"
            disabled={inspection.phase === "running"}
            onClick={() => dispatch({ type: "closeInspection" })}
          >
            Close
          </button>
        </div>
      </div>
    </section>
  )
}
