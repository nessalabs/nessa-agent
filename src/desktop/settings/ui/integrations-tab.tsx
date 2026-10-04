import {
  createContext,
  useContext,
  useEffect,
  useId,
  useReducer,
  useRef,
  type ReactNode,
} from "react"
import type { McpServersGateway } from "../adapters/mcp-servers-gateway"
import {
  canInspect,
  canWrite,
  formReady,
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
 * desktop app until #248, the sample preview — the page keeps its pending row.
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

function ManagedServers({ gateway }: { gateway: McpServersGateway }) {
  const [state, dispatch] = useReducer(
    mcpServersReducer,
    gateway.limits,
    initialMcpServersState,
  )
  useEffect(() => gateway.follow(dispatch), [gateway])
  useRequests(gateway, state, dispatch)
  const phase =
    state.access === "notAdmin"
      ? "not-admin"
      : state.list.phase === "listed"
        ? "listed"
        : state.list.phase === "notConfigured"
          ? "not-configured"
          : state.list.phase === "failed"
            ? "failed"
            : "loading"
  return (
    <div
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

function Notice({ children }: { children: ReactNode }) {
  return (
    <p className="settings-notice" role="status" data-mcp-notice>
      {children}
    </p>
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
  const unreachable =
    state.connection === "unreachable" ? (
      <Notice>
        <span data-mcp-unreachable>{sentences.unreachable}</span>
      </Notice>
    ) : null
  const notice = state.notice ? <Notice>{state.notice}</Notice> : null
  const list = state.list
  if (list.phase === "notConfigured")
    return (
      <>
        {unreachable}
        <div className="settings-empty">
          <DesktopIcon name="connections" />
          <p>{sentences.notConfigured}</p>
        </div>
      </>
    )
  if (list.phase === "failed")
    return (
      <>
        {unreachable}
        {notice}
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
  const writable = canWrite(state) && state.form === null
  const add = (
    <button
      type="button"
      className="settings-button"
      disabled={!writable}
      onClick={() => dispatch({ type: "add" })}
    >
      Add server…
    </button>
  )
  if (list.phase === "loading")
    return (
      <>
        {unreachable}
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
      {unreachable}
      {notice}
      {stored.length === 0 ? (
        <div className="settings-empty" data-mcp-empty>
          <DesktopIcon name="connections" />
          <p>{sentences.empty}</p>
          {add}
        </div>
      ) : (
        <>
          {stored.map((server) => (
            <ServerRow
              key={server.name}
              server={server}
              state={state}
              dispatch={dispatch}
            />
          ))}
          <div className="settings-row settings-servers-add">{add}</div>
        </>
      )}
      {managed.map((server) => (
        <ServerRow key={server.name} server={server} state={state} dispatch={dispatch} />
      ))}
    </>
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
  const writable = canWrite(state) && state.form === null
  const confirming = state.confirming === server.name
  const command = [server.command, ...server.args].join(" ")
  return (
    <div
      className="settings-row settings-server"
      data-mcp-server={server.name}
      data-managed={server.managed || undefined}
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
          <p className="settings-server-confirm" data-mcp-confirm>
            {sentences.removeAsk(server.name)}
          </p>
        ) : null}
      </div>
      <div className="settings-row-control settings-server-actions">
        {server.managed ? null : confirming ? (
          <>
            <button
              type="button"
              className="settings-button"
              disabled={state.pending !== null}
              onClick={() => dispatch({ type: "cancelRemove" })}
            >
              Cancel
            </button>
            <button
              type="button"
              className="settings-button settings-button-danger"
              disabled={!canWrite(state)}
              onClick={() => dispatch({ type: "confirmRemove" })}
            >
              Remove
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="settings-button"
              aria-describedby={nameId}
              disabled={!writable}
              onClick={() => dispatch({ type: "edit", name: server.name })}
            >
              Edit
            </button>
            <button
              type="button"
              className="settings-button"
              aria-describedby={nameId}
              disabled={!canInspect(state)}
              onClick={() => dispatch({ type: "inspect", name: server.name })}
            >
              Inspect
            </button>
            <button
              type="button"
              className="settings-button"
              aria-describedby={nameId}
              disabled={!writable}
              onClick={() => dispatch({ type: "askRemove", name: server.name })}
            >
              Remove
            </button>
          </>
        )}
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

function FieldProblem({ form, field }: { form: ServerForm; field: FormField }) {
  if (form.problem?.field !== field) return null
  return (
    <p className="settings-field-problem" role="alert" data-mcp-problem={field}>
      {form.problem.text}
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
  const ids = { name: useId(), command: useId(), args: useId() }
  const busy = state.pending !== null
  const change = (patch: Extract<McpServersEvent, { type: "change" }>["patch"]) =>
    dispatch({ type: "change", patch })
  return (
    <section className="settings-group" data-mcp-form={form.editing ?? ""}>
      <h2>{form.editing === undefined ? "Add server" : `Edit “${form.editing}”`}</h2>
      <fieldset className="settings-card settings-form" disabled={busy}>
        <FieldProblem form={form} field="form" />
        <div className="settings-field">
          <label htmlFor={ids.name}>Name</label>
          <input
            id={ids.name}
            className="settings-input"
            value={form.name}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={form.problem?.field === "name" || undefined}
            onChange={(event) => change({ name: event.target.value })}
          />
          <FieldProblem form={form} field="name" />
        </div>
        <div className="settings-field">
          <label htmlFor={ids.command}>Command</label>
          <input
            id={ids.command}
            className="settings-input settings-input-mono"
            value={form.command}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={form.problem?.field === "command" || undefined}
            onChange={(event) => change({ command: event.target.value })}
          />
          <FieldProblem form={form} field="command" />
        </div>
        <div className="settings-field">
          <label htmlFor={ids.args}>Arguments, one per line</label>
          <textarea
            id={ids.args}
            className="settings-input settings-input-mono"
            rows={3}
            value={form.args}
            spellCheck={false}
            aria-invalid={form.problem?.field === "args" || undefined}
            onChange={(event) => change({ args: event.target.value })}
          />
          <FieldProblem form={form} field="args" />
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
              <input
                className="settings-input settings-input-mono"
                type="password"
                aria-label={`Value of ${row.name || "the variable"}`}
                placeholder={row.stored ? sentences.storedValue : "Value"}
                value={row.value}
                autoComplete="off"
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
          <FieldProblem form={form} field="env" />
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
            onClick={() => dispatch({ type: "cancelForm" })}
          >
            Cancel
          </button>
          <button
            type="button"
            className="settings-button settings-button-primary"
            disabled={!formReady(form) || !canWrite(state)}
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
  if (!inspection) return null
  return (
    <section
      className="settings-group"
      data-mcp-inspection={inspection.phase}
      aria-live="polite"
    >
      <h2>Inspect “{inspection.name}”</h2>
      <div className="settings-card settings-inspection">
        {inspection.phase === "running" ? (
          <p className="settings-inspection-note">
            {sentences.inspecting(inspection.name, state.limits.inspectDeadlineMs)}
          </p>
        ) : inspection.phase === "failed" ? (
          <p className="settings-inspection-note" data-mcp-inspection-failure>
            {inspection.text}
          </p>
        ) : (
          <>
            {inspection.result.tools.length === 0 ? (
              <p className="settings-inspection-note">It offers no tools.</p>
            ) : (
              <ul className="settings-tools">
                {inspection.result.tools.map((tool) => (
                  <li key={tool.name} data-mcp-tool={tool.name}>
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
            {inspection.result.cut ? (
              <p
                className="settings-inspection-note"
                data-mcp-cut={inspection.result.cut}
              >
                {sentences.cut[inspection.result.cut]}
              </p>
            ) : null}
          </>
        )}
        <div className="settings-form-actions">
          <button
            type="button"
            className="settings-button"
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
