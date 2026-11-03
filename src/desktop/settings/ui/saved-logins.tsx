/**
 * Saved logins: the agents' sign-ins the gateway keeps in its vault and hands
 * to an agent it starts on another computer (issue #1260). Settings ›
 * Connections › Accounts lists one row per agent, to save, replace or remove
 * its login; a paired computer's page in Devices has its own "Use my saved
 * logins here" switch. A login is never shown again: the rows say only
 * whether one is saved.
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useState,
  type ReactNode,
} from "react"
import type { LoginAgent } from "@nessa/client"
import type {
  SavedLogins,
  SavedLoginsGateway,
  SavedLoginsOutcome,
} from "../adapters/saved-logins-gateway"
import { useVaultUnlock } from "../../../vault"
import { AgentMark } from "../../../onboarding/ui/agent-mark"
import { ItemRow, RowAction, SettingGroup, Toggle } from "./settings-controls"

const SavedLoginsContext = createContext<SavedLoginsGateway | undefined>(undefined)

export function SavedLoginsProvider({
  gateway,
  children,
}: {
  gateway: SavedLoginsGateway | undefined
  children: ReactNode
}) {
  return (
    <SavedLoginsContext.Provider value={gateway}>{children}</SavedLoginsContext.Provider>
  )
}

/** The saved logins as last read, the change in flight, and why the last one was refused. */
function useSavedLogins() {
  const gateway = useContext(SavedLoginsContext)
  const ensure = useVaultUnlock()
  const [logins, setLogins] = useState<SavedLogins | undefined>(undefined)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const take = useCallback((outcome: SavedLoginsOutcome) => {
    if (outcome.kind === "read") {
      setLogins(outcome.logins)
      setError(null)
    } else setError(outcome.sentence)
  }, [])
  useEffect(() => {
    if (!gateway) return
    let current = true
    void gateway.read().then((outcome) => current && take(outcome))
    return () => {
      current = false
    }
  }, [gateway, take])
  const act = async (
    work: (gateway: SavedLoginsGateway) => Promise<SavedLoginsOutcome>,
  ): Promise<boolean> => {
    if (!gateway || busy) return false
    setBusy(true)
    try {
      if (!(await ensure())) {
        setError("Unlock Nessa to change saved logins.")
        return false
      }
      const outcome = await work(gateway)
      take(outcome)
      return outcome.kind === "read"
    } finally {
      setBusy(false)
    }
  }
  return { gateway, logins, busy, error, act }
}

const agents: readonly {
  id: LoginAgent
  name: string
  kind: string
  placeholder: string
}[] = [
  {
    id: "claude",
    name: "Claude Code",
    kind: "Token from claude setup-token, or an Anthropic API key",
    placeholder: "sk-ant-…",
  },
  { id: "codex", name: "Codex", kind: "OpenAI API key", placeholder: "sk-…" },
  { id: "opencode", name: "OpenCode", kind: "OpenCode API key", placeholder: "…" },
]

/** Settings › Connections › Accounts › Saved logins: one row per agent. */
export function SavedLoginsGroup() {
  const state = useSavedLogins()
  return (
    <SettingGroup
      id="saved-logins"
      pending={!state.gateway}
      note="Agents on your SSH hosts and Boat workspaces sign in with these, and on a paired computer once you turn it on in Devices. A computer's own login wins."
    >
      {agents.map((agent) => (
        <SavedLoginRow key={agent.id} agent={agent} state={state} />
      ))}
      {state.error && (
        <p role="alert" className="settings-saved-logins-error">
          {state.error}
        </p>
      )}
    </SettingGroup>
  )
}

function SavedLoginRow({
  agent,
  state,
}: {
  agent: (typeof agents)[number]
  state: ReturnType<typeof useSavedLogins>
}) {
  const inputId = useId()
  const [login, setLogin] = useState("")
  const saved = state.logins?.saved[agent.id] === true
  const { busy, gateway, act } = state
  return (
    <ItemRow
      label={agent.name}
      leading={
        <span className="settings-tile">
          <AgentMark id={agent.id} name={agent.name} />
        </span>
      }
      data-saved-login={agent.id}
      control={
        saved ? (
          <span
            className="settings-boat-status"
            role="status"
            data-saved-login-state="saved"
          >
            Saved{" "}
            <RowAction
              disabled={busy}
              onClick={() => void act((gateway) => gateway.remove(agent.id))}
            >
              Remove
            </RowAction>
          </span>
        ) : undefined
      }
    >
      <div className="settings-boat">
        <form
          onSubmit={(event) => {
            event.preventDefault()
            const typed = login.trim()
            if (!typed) return
            void act((gateway) => gateway.save(agent.id, typed)).then(
              (done) => done && setLogin(""),
            )
          }}
        >
          <label htmlFor={inputId}>{agent.kind}</label>
          <div className="settings-boat-fields">
            <input
              id={inputId}
              className="settings-input settings-input-mono"
              type="password"
              autoComplete="off"
              spellCheck={false}
              placeholder={agent.placeholder}
              value={login}
              onChange={(event) => setLogin(event.target.value)}
              disabled={busy || !gateway}
              data-saved-login-input={agent.id}
            />
            <button
              className="settings-button"
              type="submit"
              disabled={!login.trim() || busy || !gateway}
              data-saved-login-save={agent.id}
            >
              {saved ? "Replace" : "Save"}
            </button>
          </div>
        </form>
      </div>
    </ItemRow>
  )
}

/**
 * A paired computer's "Use my saved logins here" switch, as one fact of its
 * page in Devices (a `<dl>` entry). Off until turned on; applies from the
 * next agent started there. Nothing without a gateway.
 */
export function DeviceLoginsSwitch({ environment }: { environment: string }) {
  const { gateway, logins, busy, error, act } = useSavedLogins()
  if (!gateway) return null
  const on = logins?.devicesOn.has(environment) === true
  return (
    <div data-device-logins={on ? "on" : "off"}>
      <dt>Use my saved logins here</dt>
      <dd>
        <Toggle
          label="Use my saved logins here"
          checked={on}
          // Unread (a locked vault), it shows off and still works: the
          // change unlocks first, and the gateway answers how it stands.
          disabled={busy}
          onChange={(next) => void act((gateway) => gateway.setDevice(environment, next))}
        />
        {error && <p role="alert">{error}</p>}
      </dd>
    </div>
  )
}
