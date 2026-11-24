/**
 * Saved logins: the agents' sign-ins the gateway keeps in its vault and hands
 * to an agent it starts on another computer (issue #1260). Settings ›
 * Connections › Accounts lists one row per agent, to save, replace or remove
 * its login; a paired computer's page in Devices has its own "Use my saved
 * logins here" switch. A login is never shown again: the rows say only
 * whether one is saved.
 *
 * Codex's row also lists its ChatGPT accounts, when the gateway keeps them:
 * "Continue with ChatGPT" starts a sign-in, the row links to it in the
 * browser, and reads again every `pollMs` until it ends. One account is in
 * use; another can be chosen, signed in again, or removed.
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import type { LoginAgent } from "@nessa/client"
import {
  chatgptEndSentence,
  type ChatgptView,
  type SavedLogins,
  type SavedLoginsGateway,
  type SavedLoginsOutcome,
} from "../adapters/saved-logins-gateway"
import { useVaultSettled, useVaultUnlock } from "../../../vault"
import { AgentMark } from "../../../onboarding/ui/agent-mark"
import { ItemRow, RowAction, RowLink, SettingGroup, Toggle } from "./settings-controls"

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

/** How often the logins are read while a ChatGPT sign-in waits for the browser. */
export const CHATGPT_POLL_MS = 2000

/** The saved logins as last read, the change in flight, and why the last one was refused. */
function useSavedLogins(pollMs = CHATGPT_POLL_MS) {
  const gateway = useContext(SavedLoginsContext)
  const ensure = useVaultUnlock()
  const vaultSettled = useVaultSettled()
  const reads = useRef(0)
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
    const revision = ++reads.current
    void gateway.read().then((outcome) => {
      if (current && revision === reads.current) take(outcome)
    })
    return () => {
      current = false
    }
  }, [gateway, take, vaultSettled])
  // While a ChatGPT sign-in waits for the browser, how it ends is read again.
  // Polls are serialized so a slow gateway read cannot be starved by the next tick.
  const waiting = logins?.chatgpt?.signIn.kind === "waiting"
  useEffect(() => {
    if (!gateway || !waiting) return
    let current = true
    let timer: number | undefined
    const schedule = () => {
      timer = window.setTimeout(() => {
        const revision = ++reads.current
        void gateway
          .read()
          .then((outcome) => {
            if (current && revision === reads.current) take(outcome)
          })
          .finally(() => {
            if (current) schedule()
          })
      }, pollMs)
    }
    schedule()
    return () => {
      current = false
      if (timer !== undefined) window.clearTimeout(timer)
    }
  }, [gateway, waiting, take, pollMs])
  /**
   * Unlocks first, then does `work`. What is shown may be unread (a locked
   * vault refused the read on mount): then it is read again once unlocked,
   * and `work` is given that, so a change is made from how things stand.
   */
  const act = async (
    work: (
      gateway: SavedLoginsGateway,
      current: SavedLogins | undefined,
    ) => Promise<SavedLoginsOutcome>,
  ): Promise<boolean> => {
    if (!gateway || busy) return false
    reads.current += 1
    setBusy(true)
    try {
      if (!(await ensure())) {
        setError("Unlock Nessa to change saved logins.")
        return false
      }
      let current = logins
      if (!current) {
        const read = await gateway.read()
        if (read.kind === "read") {
          current = read.logins
          reads.current += 1
          setLogins(read.logins)
        }
      }
      reads.current += 1
      const outcome = await work(gateway, current)
      reads.current += 1
      take(outcome)
      if (outcome.kind === "refused") {
        // A refusal can follow a change that was made (its record could
        // not be kept): show how things stand, keeping why.
        const after = await gateway.read()
        if (after.kind === "read") {
          reads.current += 1
          setLogins(after.logins)
        }
      }
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
export function SavedLoginsGroup({ pollMs }: { pollMs?: number } = {}) {
  const state = useSavedLogins(pollMs)
  return (
    <SettingGroup
      id="saved-logins"
      pending={!state.gateway}
      note="Agents on your SSH hosts and cloud workspaces (Boat, Agent37) sign in with these, and on a paired computer once you turn it on in Devices. A computer's own login wins."
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
        {agent.id === "codex" && state.logins?.chatgpt && (
          <ChatgptAccounts chatgpt={state.logins.chatgpt} state={state} />
        )}
      </div>
    </ItemRow>
  )
}

/**
 * Codex's ChatGPT accounts under its row: the one in use, the others to
 * choose, and the sign-in. Codex on other computers uses the account in
 * use in place of the key above; with none in use, the key.
 */
function ChatgptAccounts({
  chatgpt,
  state,
}: {
  chatgpt: ChatgptView
  state: ReturnType<typeof useSavedLogins>
}) {
  const { busy, act } = state
  const { signIn, accounts } = chatgpt
  const ended =
    signIn.kind === "idle" && signIn.last ? chatgptEndSentence(signIn.last) : undefined
  const anyInUse = accounts.some((account) => account.inUse)
  return (
    <div className="settings-chatgpt" data-chatgpt-accounts={accounts.length}>
      <p>
        Or use your ChatGPT plan: Codex on your SSH hosts and cloud workspaces then runs
        on the account in use instead of the key.
      </p>
      {accounts.length > 0 && (
        <ul className="settings-chatgpt-accounts">
          {accounts.map((account) => (
            <li
              key={account.id}
              data-chatgpt-account={account.email || account.id}
              data-chatgpt-in-use={account.inUse ? "" : undefined}
            >
              <span className="settings-chatgpt-email">
                {account.email || "ChatGPT account"}
              </span>
              <span className="settings-chatgpt-state" role="status">
                {account.needsSignIn
                  ? "Needs signing in again"
                  : account.inUse
                    ? "In use"
                    : ""}
              </span>
              <span className="settings-chatgpt-actions">
                {account.needsSignIn ? (
                  <RowAction
                    disabled={busy || signIn.kind === "waiting"}
                    onClick={() =>
                      void act((gateway) => gateway.chatgptSignIn(account.id))
                    }
                    data-chatgpt-sign-in-again={account.id}
                  >
                    Sign in again
                  </RowAction>
                ) : account.inUse ? (
                  <RowAction
                    disabled={busy}
                    onClick={() => void act((gateway) => gateway.chatgptUse(null))}
                    data-chatgpt-stop-using={account.id}
                  >
                    Use the key
                  </RowAction>
                ) : (
                  <RowAction
                    disabled={busy}
                    onClick={() => void act((gateway) => gateway.chatgptUse(account.id))}
                    data-chatgpt-use={account.id}
                  >
                    Use
                  </RowAction>
                )}
                <RowAction
                  disabled={busy}
                  onClick={() => void act((gateway) => gateway.chatgptRemove(account.id))}
                  data-chatgpt-remove={account.id}
                >
                  Remove
                </RowAction>
              </span>
            </li>
          ))}
        </ul>
      )}
      {signIn.kind === "waiting" ? (
        <div className="settings-chatgpt-waiting" data-chatgpt-sign-in="waiting">
          <p role="status">Finish signing in to ChatGPT in your browser.</p>
          <div className="settings-boat-fields">
            <RowLink href={signIn.authorizeUrl} data-chatgpt-open>
              Open ChatGPT
            </RowLink>
            <RowAction
              disabled={busy}
              onClick={() => void act((gateway) => gateway.chatgptCancel())}
              data-chatgpt-cancel
            >
              Cancel
            </RowAction>
          </div>
        </div>
      ) : (
        <div className="settings-boat-fields" data-chatgpt-sign-in="idle">
          <RowAction
            disabled={busy}
            onClick={() => void act((gateway) => gateway.chatgptSignIn())}
            data-chatgpt-continue
          >
            {accounts.length > 0
              ? "Add another ChatGPT account"
              : "Continue with ChatGPT"}
          </RowAction>
          {accounts.length > 0 && !anyInUse && (
            <span className="settings-chatgpt-note">Codex uses the key above.</span>
          )}
        </div>
      )}
      {ended && (
        <p
          role="status"
          data-chatgpt-ended={signIn.kind === "idle" ? signIn.last : undefined}
        >
          {ended}
        </p>
      )}
    </div>
  )
}

/**
 * A paired computer's "Use my saved logins here" switch, as one fact of its
 * page in Devices (a `<dl>` entry). Off until turned on; applies to
 * agents opened there after it. Nothing without a gateway.
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
          onChange={(next) =>
            void act((gateway, current) =>
              // Read only once unlocked: turned from how it stands, not from
              // the "off" an unread switch shows.
              gateway.setDevice(
                environment,
                current && !logins ? !current.devicesOn.has(environment) : next,
              ),
            )
          }
        />
        {error && <p role="alert">{error}</p>}
      </dd>
    </div>
  )
}
