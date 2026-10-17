import * as React from "react"
import {
  AgentActionButton,
  AgentActionCard,
} from "../../provider-authentication/ui/agent-action-card"
import type { AgentInstallations } from "../application/agent-installations"
import type {
  AgentApiKeySink,
  AgentReadinessSource,
  ApiKeyAgent,
} from "../application/ports"
import { AGENT_CHOICES, type AgentId, type AgentReadiness } from "../model/onboarding"
import { installationOutcomeText } from "./agent-downloads"
import { AgentDownloadProgress } from "./agent-download-progress"
import { useAgentDownloads } from "./use-agent-downloads"
import { AgentApiKeyForm } from "./agent-api-key-form"
import { AgentApiKeyAuditWarning, useAgentApiKeySave } from "./agent-api-key-save"

export interface AgentSetupSource {
  readonly apiKeys?: AgentApiKeySink
  readonly readiness: AgentReadinessSource
  readonly installations: AgentInstallations
  readonly signIn: (agent: "claude" | "codex") => Promise<void>
  readonly loginAvailable: () => Promise<boolean>
}

const Setup = React.createContext<AgentSetupSource | undefined>(undefined)

/** Native composition supplies setup; previews and remote browser views omit it. */
export function AgentSetupProvider({
  source,
  children,
}: {
  source?: AgentSetupSource
  children: React.ReactNode
}) {
  return <Setup.Provider value={source}>{children}</Setup.Provider>
}

const readinessText = (name: string): Record<AgentReadiness, string> => ({
  ready: "Ready. Send your message when you are ready.",
  "needs-authentication": "The runtime is installed. Sign in to your agent account next.",
  "not-installed": `Download ${name} to use it in Nessa.`,
  "not-configured":
    "This gateway is not configured to run this agent. Downloading a runtime alone will not change that.",
  "not-supported": "This agent is not supported by this gateway.",
  unknown: "Nessa could not check this agent. Retry before sending.",
})

/** A setup action never resends the message whose refusal opened this view. */
export function SelectedAgentSetup({
  agent,
  recovery = false,
}: {
  agent: string
  /** First-use hints follow published missing-runtime/authentication answers. */
  recovery?: boolean
}) {
  const source = React.useContext(Setup)
  const choice = AGENT_CHOICES.find((choice) => choice.id === agent)
  if (!source || !choice) return null
  return (
    <SelectedSetup
      key={choice.id}
      agent={choice.id}
      name={choice.name}
      source={source}
      recovery={recovery}
    />
  )
}

function SelectedSetup({
  agent,
  name,
  source,
  recovery,
}: {
  agent: AgentId
  name: string
  source: AgentSetupSource
  recovery: boolean
}) {
  const credentials = useAgentApiKeySave(source.apiKeys)
  const [readiness, setReadiness] = React.useState<AgentReadiness>("unknown")
  const [checking, setChecking] = React.useState(true)
  // Undefined until the host answers, so neither sign-in nor key entry flashes.
  const [loginAvailable, setLoginAvailable] = React.useState<boolean>()
  const [signingIn, setSigningIn] = React.useState(false)
  const [loginFailed, setLoginFailed] = React.useState(false)
  const loginWorking = React.useRef(false)
  const generation = React.useRef(0)
  const revision = React.useRef(0)
  const check = React.useCallback(async () => {
    const request = ++revision.current
    const lifetime = generation.current
    setChecking(true)
    const answer = await source.readiness.read().catch(() => ({ ok: false as const }))
    if (lifetime !== generation.current || request !== revision.current) return
    setReadiness(
      answer.ok && Object.hasOwn(answer.agents, agent)
        ? (answer.agents[agent] ?? "unknown")
        : "unknown",
    )
    setChecking(false)
    // Login-launch availability is independent of a runtime readiness answer.
    void source
      .loginAvailable()
      .catch(() => false)
      .then((available) => {
        if (lifetime === generation.current && request === revision.current)
          setLoginAvailable(available)
      })
  }, [source, agent])
  const downloads = useAgentDownloads(source.installations, () => void check())
  React.useEffect(() => {
    generation.current += 1
    setReadiness("unknown")
    loginWorking.current = false
    setSigningIn(false)
    setLoginFailed(false)
    void check()
    return () => {
      generation.current += 1
    }
  }, [check])
  const offer = downloads.offers?.find((offer) => offer.agent === agent)
  const retainedResult =
    downloads.outcome?.status === "failed" ||
    (downloads.outcome?.status === "installed" && downloads.outcome.cleanupPending)
  if (
    !recovery &&
    readiness !== "not-installed" &&
    readiness !== "needs-authentication" &&
    downloads.pending === undefined &&
    !retainedResult &&
    !credentials.auditFailure &&
    !loginFailed
  )
    return null
  // OpenCode signs in with a key only. Claude takes one too, so where this host
  // cannot open Claude's own sign-in the key is the way to make it ready.
  const keyAgent: ApiKeyAgent | undefined =
    agent === "opencode" || (agent === "claude" && loginAvailable === false)
      ? agent
      : undefined
  const primary =
    readiness === "not-installed" && offer && !offer.installed ? (
      <AgentActionButton
        disabled={downloads.pending !== undefined || checking}
        onClick={() => downloads.install(agent)}
      >
        {downloads.pending
          ? "Downloading…"
          : `Download ${name} · ${Math.ceil(offer.archiveBytes / 1_000_000)} MB`}
      </AgentActionButton>
    ) : readiness === "needs-authentication" &&
      agent !== "opencode" &&
      loginAvailable === true ? (
      <AgentActionButton
        disabled={signingIn}
        onClick={async () => {
          if (loginWorking.current) return
          loginWorking.current = true
          const lifetime = generation.current
          setSigningIn(true)
          setLoginFailed(false)
          try {
            await source.signIn(agent)
          } catch {
            if (generation.current === lifetime) setLoginFailed(true)
          }
          if (generation.current !== lifetime) return
          loginWorking.current = false
          setSigningIn(false)
          void check()
        }}
      >
        {signingIn ? "Opening sign-in…" : `Sign in to ${name}`}
      </AgentActionButton>
    ) : null
  return (
    <section aria-label={`${name} setup`} className="mb-2">
      <AgentActionCard
        label={`${name} setup steps`}
        title={
          <span role="status" aria-live="polite">
            {downloads.pending ? (
              <AgentDownloadProgress name={name} archiveBytes={offer?.archiveBytes} />
            ) : checking ? (
              `Checking ${name} setup…`
            ) : (
              readinessText(name)[readiness]
            )}
          </span>
        }
        actions={
          <>
            {primary}
            <AgentActionButton
              quiet={primary !== null}
              disabled={checking || downloads.pending !== undefined}
              onClick={() => {
                downloads.refresh()
                void check()
              }}
            >
              Retry
            </AgentActionButton>
          </>
        }
      >
        {downloads.outcome ? <p>{installationOutcomeText(downloads.outcome)}</p> : null}
        {downloads.unavailable ? (
          <p>Downloads could not be checked. Retry to check them again.</p>
        ) : null}
        {readiness === "needs-authentication" && source.apiKeys && keyAgent ? (
          <AgentApiKeyForm
            agent={keyAgent}
            agentName={keyAgent === "opencode" ? "OpenCode" : name}
            hint={
              keyAgent === "opencode"
                ? "OpenCode connects through Zen using your API key. Depending on the configured model, messages may be metered."
                : undefined
            }
            onSave={credentials.save}
            onSaved={check}
          />
        ) : null}
        <AgentApiKeyAuditWarning failure={credentials.auditFailure} />
        {loginFailed ? <p>Sign-in could not be opened. Try again.</p> : null}
      </AgentActionCard>
    </section>
  )
}
