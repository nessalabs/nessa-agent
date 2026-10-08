import {
  createContext,
  useContext,
  useEffect,
  useId,
  useReducer,
  useRef,
  type KeyboardEvent,
  type DragEvent,
  type ClipboardEvent,
  type ReactNode,
} from "react"
import type { McpServersGateway } from "../adapters/mcp-servers-gateway"
import {
  canInspect,
  canRemoveByName,
  canWrite,
  editedServer,
  endsWithLineBreak,
  authorizationLabel,
  formReady,
  groupsOf,
  hasLineBreak,
  launchChanged,
  lineBreaks,
  linesOf,
  namesKept,
  valuesNeeded,
  initialMcpServersState,
  mcpServersReducer,
  sentences,
  type FormField,
  type InspectedTool,
  type ListedServer,
  type McpServersEvent,
  type McpServersState,
  type ServerForm,
  type ServerGroup,
  type VariableRow,
} from "../model/mcp-servers"
import { DesktopIcon } from "../../ui/icons"
import { ItemRow, PendingAction, SettingGroup, Toggle } from "./settings-controls"

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
 * row's or group's button that opened it, or on Add (`focusAfter`). Escape
 * closes the form and the confirm.
 *
 * A name stored more than once is one group, read-only, whose one action
 * removes the first stored under it: the gateway finds a server by name, so
 * no other could be targeted (G3).
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
      <SettingGroup id="mcp-servers" note={footnote} pending>
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
          : pending.kind === "remove"
            ? gateway.remove(pending.request)
            : pending.kind === "authorize"
              ? gateway.authorize(pending.id, pending.revision)
              : gateway.revoke(pending.id, pending.revision)
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

/**
 * A button focus may go back to: a row's or a group's by its server's name,
 * or one of the tab's own.
 */
export interface FocusTarget {
  readonly action: "edit" | "inspect" | "remove" | "removeFirst" | "add" | "removeByName"
  readonly server?: string
}

/**
 * Where focus goes back to when the form, the confirm or the inspection
 * closes between `previous` and `next`: the first of these drawn, once it is
 * enabled. `null` when nothing closed. A saved form goes back to the row
 * under the name saved, a cancelled one to the row it edited. A confirm goes
 * back to the group it was asked from while the name is still shared, else
 * to the one row left under it, else to Add (G6).
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
      { action: "removeFirst", server: previous.confirming.name },
      { action: "remove", server: previous.confirming.name },
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
      : [...container.querySelectorAll("[data-mcp-server], [data-mcp-group]")].find(
          (row) =>
            (row.getAttribute("data-mcp-server") ??
              row.getAttribute("data-mcp-group")) === target.server,
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
      <SettingGroup id="mcp-servers" note={footnote}>
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
        <div className="settings-skeleton" data-mcp-skeleton aria-hidden="true">
          <span />
        </div>
        <div className="settings-skeleton" data-mcp-skeleton aria-hidden="true">
          <span />
        </div>
        <div className="settings-servers-add">{add}</div>
      </>
    )
  const { groups, managed } = groupsOf(list)
  return (
    <>
      {groups.length === 0 ? (
        <div className="settings-empty" data-mcp-empty>
          <DesktopIcon name="connections" />
          <p>{sentences.empty}</p>
          {add}
        </div>
      ) : (
        <>
          {/* Keyed by occurrence id, which a row keeps across reloads (G1). */}
          {groups.map((group) =>
            group.rows.length === 1 ? (
              <ServerRow
                key={group.rows[0].id}
                server={group.rows[0].server}
                state={state}
                dispatch={dispatch}
              />
            ) : (
              <SharedGroup
                key={group.rows[0].id}
                group={group}
                state={state}
                dispatch={dispatch}
              />
            ),
          )}
          <div className="settings-servers-add">{add}</div>
        </>
      )}
      {managed.map((row) => (
        <ServerRow key={row.id} server={row.server} state={state} dispatch={dispatch} />
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
    <ItemRow
      className="settings-server"
      data-mcp-too-large
      onKeyDown={onKeyDown}
      label={<span id={textId}>{sentences.listTooLarge}</span>}
      control={
        <span className="settings-server-actions">
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
        </span>
      }
    >
      <div className="settings-field">
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
      </div>
      {state.confirming !== null ? (
        <p id={askId} className="settings-server-confirm" data-mcp-confirm>
          {sentences.removeByNameAsk(state.confirming.name)}
        </p>
      ) : null}
    </ItemRow>
  )
}

/** The confirm's two buttons, keyed: they are not the row's, reused by place. */
function ConfirmButtons({
  askId,
  state,
  dispatch,
}: {
  askId: string
  state: McpServersState
  dispatch: Dispatch
}) {
  const cancel = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    cancel.current?.focus()
  }, [])
  return (
    <>
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
      </button>
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
      </button>
    </>
  )
}

/** The confirm's Escape: Settings' own never closes it. */
function confirmEscape(confirming: boolean, state: McpServersState, dispatch: Dispatch) {
  return (event: KeyboardEvent) => {
    if (!confirming || event.key !== "Escape" || state.pending !== null) return
    event.preventDefault()
    event.stopPropagation()
    dispatch({ type: "cancelRemove" })
  }
}

/** A server stored once under its name, or the managed one (G2). */
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
  const writable = canWrite(state) && state.form === null
  const confirming = !server.managed && state.confirming?.name === server.name
  const command = server.url ?? [server.command, ...server.args].join(" ")
  const authorization = authorizationLabel(server)
  const consent = state.consent?.name === server.name ? state.consent : null
  return (
    <ItemRow
      className="settings-server"
      data-mcp-server={server.name}
      data-managed={server.managed || undefined}
      onKeyDown={confirmEscape(confirming, state, dispatch)}
      label={<span id={nameId}>{server.name}</span>}
      detail={
        <>
          <code className="settings-server-command">{command}</code>
          <small>
            {server.managed
              ? sentences.managed
              : server.url
                ? (authorization ?? server.url)
                : sentences.variables(server.envNames.length)}
          </small>
        </>
      }
      control={
        <span className="settings-server-actions">
          {server.managed ? null : confirming ? (
            <ConfirmButtons
              key="confirm"
              askId={askId}
              state={state}
              dispatch={dispatch}
            />
          ) : (
            [
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
              ...(server.remoteId
                ? [
                    <button
                      key="authorize"
                      type="button"
                      className="settings-button"
                      data-mcp-action="authorize"
                      aria-describedby={nameId}
                      disabled={!writable}
                      onClick={() => dispatch({ type: "authorize", name: server.name })}
                    >
                      Authorize
                    </button>,
                    <button
                      key="revoke"
                      type="button"
                      className="settings-button"
                      data-mcp-action="revoke"
                      aria-describedby={nameId}
                      disabled={!writable}
                      onClick={() => dispatch({ type: "revoke", name: server.name })}
                    >
                      Revoke
                    </button>,
                  ]
                : []),
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
            ]
          )}
          <Toggle
            checked={server.enabled}
            label={server.name}
            disabled={server.managed || !writable || confirming}
            onChange={() => dispatch({ type: "toggle", name: server.name })}
          />
        </span>
      }
    >
      {consent ? (
        <a href={consent.url} data-mcp-consent>
          {sentences.pendingConsent}
        </a>
      ) : null}
      {confirming ? (
        <p id={askId} className="settings-server-confirm" data-mcp-confirm>
          {sentences.removeAsk(server.name)}
        </p>
      ) : null}
    </ItemRow>
  )
}

/**
 * A name stored more than once (G3): its servers read-only, how many, and
 * one action, which removes the first stored under it — the gateway's only
 * way to address any of them.
 */
function SharedGroup({
  group,
  state,
  dispatch,
}: {
  group: ServerGroup
  state: McpServersState
  dispatch: Dispatch
}) {
  const sharedId = useId()
  const askId = useId()
  const writable = canWrite(state) && state.form === null
  const confirming = state.confirming?.name === group.name
  return (
    <ItemRow
      className="settings-server settings-server-group"
      data-mcp-group={group.name}
      onKeyDown={confirmEscape(confirming, state, dispatch)}
      label={group.name}
      detail={
        <small id={sharedId} data-mcp-shared>
          {sentences.nameShared(group.rows.length)}
        </small>
      }
      control={
        <span className="settings-server-actions">
          {confirming ? (
            <ConfirmButtons
              key="confirm"
              askId={askId}
              state={state}
              dispatch={dispatch}
            />
          ) : (
            <button
              key="removeFirst"
              type="button"
              className="settings-button settings-button-wrapped"
              data-mcp-action="removeFirst"
              aria-describedby={sharedId}
              disabled={!writable}
              onClick={() => dispatch({ type: "askRemove", name: group.name })}
            >
              {sentences.removeFirst(group.name)}
            </button>
          )}
        </span>
      }
    >
      <ul className="settings-server-shared">
        {group.rows.map(({ id, server }) => (
          <li key={id} data-mcp-shared-row>
            <code className="settings-server-command">
              {[server.command, ...server.args].join(" ")}
            </code>
            <small>
              {[
                sentences.variables(server.envNames.length),
                ...(server.enabled ? [] : [sentences.notOffered]),
              ].join(" · ")}
            </small>
          </li>
        ))}
      </ul>
      {confirming ? (
        <p id={askId} className="settings-server-confirm" data-mcp-confirm>
          {sentences.removeFirstAsk(group.name, group.rows[0].server.command)}
        </p>
      ) : null}
    </ItemRow>
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
  const ids = {
    name: useId(),
    command: useId(),
    args: useId(),
    values: useId(),
    url: useId(),
  }
  const problems = {
    form: useId(),
    name: useId(),
    command: useId(),
    args: useId(),
    env: useId(),
    url: useId(),
  } satisfies Record<FormField, string>
  const first = useRef<HTMLInputElement>(null)
  const section = useRef<HTMLElement>(null)
  // Where focus goes once the rows an action changed are drawn (F11, F12).
  const focusNext = useRef<string | null>(null)
  const busy = state.pending !== null
  useEffect(() => {
    first.current?.focus()
  }, [])
  useEffect(() => {
    const selector = focusNext.current
    if (selector === null) return
    focusNext.current = null
    section.current?.querySelector<HTMLElement>(selector)?.focus()
  })
  /** The key the reducer hands the next row it adds. */
  const nextKey = state.rows + 1
  const argumentField = (key: number) => `[data-mcp-argument-key="${key}"] textarea`
  const variableField = (key: number) => `[data-mcp-variable-key="${key}"] input`
  const addArgument = (after?: number) => {
    focusNext.current = argumentField(nextKey)
    dispatch({ type: "addArgument", after })
  }
  const onArgumentKeyDown = (event: KeyboardEvent, key: number) => {
    // Enter is another argument; Shift+Enter a line break in this one (F11).
    if (event.key !== "Enter" || event.shiftKey || event.altKey) return
    // An IME's Enter commits its text: Safari says so only by keyCode 229 (M1).
    const composing = event.nativeEvent.isComposing || event.keyCode === 229
    if (event.metaKey || event.ctrlKey || composing) return
    event.preventDefault()
    addArgument(key)
  }
  const removeArgument = (at: number) => {
    const following = form.args[at + 1]
    focusNext.current = following
      ? argumentField(following.key)
      : '[data-mcp-action="add-argument"]'
    dispatch({ type: "removeArgument", key: form.args[at].key })
  }
  const removeVariable = (at: number) => {
    const following = form.env[at + 1]
    focusNext.current = following
      ? variableField(following.key)
      : '[data-mcp-action="add-variable"]'
    dispatch({ type: "removeVariable", key: form.env[at].key })
  }
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
  const keepable = namesKept(form, listed)
  return (
    <section
      ref={section}
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
        <div className="settings-field" role="group" aria-label="Server kind">
          <label>
            <input
              type="radio"
              name={`${ids.name}-kind`}
              data-mcp-field="kind"
              value="stdio"
              checked={form.kind === "stdio"}
              onChange={() => change({ kind: "stdio" })}
            />
            Local command
          </label>
          <label>
            <input
              type="radio"
              name={`${ids.name}-kind`}
              data-mcp-field="kind"
              value="remote"
              checked={form.kind === "remote"}
              onChange={() => change({ kind: "remote" })}
            />
            Remote URL
          </label>
        </div>
        {form.kind === "remote" ? (
          <div className="settings-field">
            <label htmlFor={ids.url}>URL</label>
            <input
              id={ids.url}
              className="settings-input"
              data-mcp-field="url"
              value={form.url}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={form.problem?.field === "url" || undefined}
              aria-describedby={described("url")}
              onChange={(event) => change({ url: event.target.value })}
            />
            <FieldProblem form={form} field="url" id={problems.url} />
          </div>
        ) : null}
        {form.kind === "stdio" ? (
          <>
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
                <ArgumentField
                  key={row.key}
                  at={at}
                  value={row.value}
                  rowKey={row.key}
                  invalid={form.problem?.field === "args"}
                  problemId={described("args")}
                  onKeyDown={(event) => onArgumentKeyDown(event, row.key)}
                  onChange={(value) =>
                    dispatch({ type: "changeArgument", key: row.key, value })
                  }
                  onRemove={() => removeArgument(at)}
                />
              ))}
              <div>
                <button
                  type="button"
                  className="settings-button"
                  data-mcp-action="add-argument"
                  onClick={() => addArgument()}
                >
                  Add argument
                </button>
              </div>
              <FieldProblem form={form} field="args" id={problems.args} />
            </div>
            <div className="settings-field">
              <span className="settings-field-label">Variables</span>
              {form.env.map((row, at) => (
                <div
                  className="settings-variable"
                  key={row.key}
                  data-mcp-variable={row.name}
                  data-mcp-variable-key={row.key}
                >
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
                  <SecretField
                    secret={secretOf(row)}
                    relaunched={relaunched}
                    keepable={keepable}
                    describedBy={
                      [described("env"), row.stored && waiting ? ids.values : undefined]
                        .filter(Boolean)
                        .join(" ") || undefined
                    }
                    dispatch={dispatch}
                    focusAfter={(selector) => {
                      focusNext.current = `[data-mcp-variable-key="${row.key}"] ${selector}`
                    }}
                  />
                  <button
                    type="button"
                    className="settings-button"
                    aria-label={`Remove ${row.name || "this variable"}`}
                    onClick={() => removeVariable(at)}
                  >
                    Remove
                  </button>
                </div>
              ))}
              <div>
                <button
                  type="button"
                  className="settings-button"
                  data-mcp-action="add-variable"
                  onClick={() => {
                    focusNext.current = variableField(nextKey)
                    dispatch({ type: "addVariable" })
                  }}
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
          </>
        ) : null}
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

/**
 * One argument: Enter adds the next (the form's `onKeyDown`), Shift+Enter
 * puts a line break in it, and a line break held is marked under it (F11).
 */
function ArgumentField({
  at,
  rowKey,
  value,
  invalid,
  problemId,
  onKeyDown,
  onChange,
  onRemove,
}: {
  at: number
  rowKey: number
  value: string
  invalid: boolean
  problemId: string | undefined
  onKeyDown: (event: KeyboardEvent) => void
  onChange: (value: string) => void
  onRemove: () => void
}) {
  const breaksId = useId()
  const breaks = lineBreaks(value)
  return (
    <div
      className="settings-argument"
      data-mcp-argument={at}
      data-mcp-argument-key={rowKey}
    >
      <div className="settings-argument-field">
        <textarea
          className="settings-input settings-input-mono settings-input-wrapped"
          rows={1}
          aria-label={`Argument ${at + 1}`}
          value={value}
          autoComplete="off"
          spellCheck={false}
          aria-invalid={invalid || undefined}
          aria-describedby={
            [problemId, breaks > 0 ? breaksId : undefined].filter(Boolean).join(" ") ||
            undefined
          }
          onKeyDown={onKeyDown}
          onChange={(event) => onChange(event.target.value)}
        />
        {breaks > 0 ? (
          <small id={breaksId} className="settings-field-note" data-mcp-argument-breaks>
            {sentences.argumentBreaks(breaks)}
          </small>
        ) : null}
      </div>
      <button
        type="button"
        className="settings-button"
        aria-label={`Remove argument ${at + 1}`}
        onClick={onRemove}
      >
        Remove
      </button>
    </div>
  )
}

/**
 * A variable's value (S1–S6). A password field: masked, kept out of the
 * accessibility tree and from copy, under Secure Event Input. Uncontrolled,
 * with no value or default given: React writes a controlled field's value
 * into the markup as its value attribute, and this field's is only ever its
 * own (U31). A paste with a line break, which a password field would drop,
 * is held instead and never drawn: the field gives way to how many lines it
 * is, a trailing line break pointed out with a one-click trim, and Clear.
 */
/**
 * What the value field draws of its row: flags and counts, never the value
 * itself, which goes no further than the model (M6).
 */
interface Secret {
  readonly key: number
  readonly name: string
  readonly stored: boolean
  readonly edited: boolean
  readonly held: boolean
  readonly empty: boolean
  /** A held value's lines, and whether it ends with a line break (S4, S5). */
  readonly lines: number
  readonly endsWithBreak: boolean
}

const secretOf = (row: VariableRow): Secret => ({
  key: row.key,
  name: row.name,
  stored: row.stored,
  edited: row.edited,
  held: row.held,
  empty: row.value === "",
  lines: row.held ? linesOf(row.value) : 0,
  endsWithBreak: row.held && endsWithLineBreak(row.value),
})

function SecretField({
  secret: row,
  relaunched,
  keepable,
  describedBy,
  dispatch,
  focusAfter,
}: {
  secret: Secret
  relaunched: boolean
  /** Whether a stored value may be kept: the command, arguments and variable names as stored (V9). */
  keepable: boolean
  describedBy: string | undefined
  dispatch: Dispatch
  /** Where focus goes once this field is drawn again, within its row. */
  focusAfter: (selector: string) => void
}) {
  const field = useRef<HTMLInputElement>(null)
  const label = `Value of ${row.name || "the variable"}`
  /** Text with a line break, pasted or dropped, which a password field would drop: held (S4, M3). */
  const hold = (event: ClipboardEvent | DragEvent, text: string) => {
    if (!hasLineBreak(text)) return
    event.preventDefault()
    focusAfter('[data-mcp-action="clear-value"]')
    dispatch({ type: "pasteVariable", key: row.key, value: text })
  }
  const keep =
    row.stored && row.edited && keepable ? (
      <button
        type="button"
        className="settings-button"
        data-mcp-action="keep-value"
        onClick={() => {
          if (field.current) field.current.value = ""
          focusAfter("[data-mcp-secret]")
          dispatch({ type: "keepVariable", key: row.key })
        }}
      >
        {sentences.keepStored}
      </button>
    ) : null
  if (row.held)
    return (
      <div
        className="settings-secret-held"
        role="group"
        aria-label={label}
        aria-describedby={describedBy}
        data-mcp-secret-held
      >
        <span data-mcp-pasted>
          {sentences.pasted(row.lines)}
          {row.endsWithBreak ? `, ${sentences.endsWithBreak}` : null}
        </span>
        {row.endsWithBreak ? (
          <button
            type="button"
            className="settings-button"
            data-mcp-action="trim-value"
            onClick={() => {
              // Focus stays: on this button while a line break is left to
              // remove, else on Clear, the next in the group (M2).
              focusAfter(
                '[data-mcp-action="trim-value"], [data-mcp-action="clear-value"]',
              )
              dispatch({ type: "trimVariable", key: row.key })
            }}
          >
            {sentences.trimBreak}
          </button>
        ) : null}
        <button
          type="button"
          className="settings-button"
          data-mcp-action="clear-value"
          onClick={() => {
            focusAfter("[data-mcp-secret]")
            dispatch({ type: "clearVariable", key: row.key })
          }}
        >
          {sentences.clearPasted}
        </button>
        {keep}
      </div>
    )
  return (
    <>
      <input
        ref={field}
        type="password"
        className="settings-input settings-input-mono"
        aria-label={label}
        aria-describedby={describedBy}
        placeholder={
          row.stored
            ? row.edited
              ? row.empty
                ? sentences.storedValueCleared
                : undefined
              : relaunched
                ? sentences.storedValueAgain
                : sentences.storedValue
            : "Value"
        }
        // Not "off", which a browser may ignore and fill a saved password into (M4).
        autoComplete="new-password"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        data-mcp-secret
        onPaste={(event) => hold(event, event.clipboardData.getData("text/plain"))}
        onDrop={(event) => hold(event, event.dataTransfer.getData("text/plain"))}
        onChange={(event) =>
          dispatch({
            type: "changeVariable",
            key: row.key,
            patch: { value: event.target.value },
          })
        }
      />
      {keep}
    </>
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
