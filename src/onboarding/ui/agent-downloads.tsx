import type {
  AgentInstallations,
  InstallationOutcome,
} from "../application/agent-installations"
import { AGENT_CHOICES, type AgentId } from "../model/onboarding"
import { useAgentDownloads } from "./use-agent-downloads"
import { LoaderCircle } from "lucide-react"

export function installationOutcomeText(
  outcome: InstallationOutcome | undefined,
): string | undefined {
  if (!outcome) return undefined
  if (outcome.status === "installed")
    return outcome.cleanupPending
      ? "Installed. Cleanup of an older runtime needs attention. Check your agent’s sign-in next."
      : "Installed. Check your agent’s sign-in next."
  switch (outcome.reason) {
    case "storage":
      return "Nessa could not save the runtime. Check available disk space and try again."
    case "refused":
      return "The release server refused this download. Check for a Nessa update or report this problem."
    case "download":
      return "The download failed. Check your connection and try again."
    case "verification":
      return "The download could not be verified. Nothing was installed. Please report this problem."
    case "busy":
      return "An installation is already running. Check downloads again shortly."
    case "unavailable":
      return "Downloads are unavailable right now. Check your connection to Nessa."
    case "not-confirmed":
      return "Installation was not confirmed. It may still be running; check downloads before trying again."
  }
}

const megabytes = (bytes: number) => `${Math.ceil(bytes / 1_000_000)} MB`

const agentName = (agent: AgentId) =>
  AGENT_CHOICES.find((choice) => choice.id === agent)?.name ?? agent

const ROW_ACTION =
  "rounded-full px-3 py-1 nessa-text-3 text-foreground bg-muted transition-[background-color,transform] duration-150 ease-out hover:bg-accent focus-visible:bg-accent active:scale-[0.97] focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50"

/**
 * Offers are unknown or stale: the gateway could not be asked, or an install
 * may still be running and needs a fresh look before another try.
 */
function CheckAgain({
  unavailable,
  refresh,
}: {
  unavailable: boolean
  refresh: () => void
}) {
  return (
    <div className="flex min-h-11 items-center gap-3 px-3.5 py-1.5 nessa-text-4">
      <span className="flex-1 text-muted-foreground">
        {unavailable ? "Can’t check downloads" : "Check again shortly"}
      </span>
      <button type="button" onClick={refresh} className={ROW_ACTION}>
        Check again
      </button>
    </div>
  )
}

/**
 * One row per agent and a word or two for where it stands: Installed,
 * Downloading, Download, or Retry. Everything longer — why a download failed,
 * what is downloading — is said to assistive technology and kept in the
 * Retry button's tooltip, not printed under the rows.
 */
export function AgentDownloadsView({
  offers,
  agents,
  pending,
  outcome,
  attempted,
  unavailable,
  refresh,
  install,
}: ReturnType<typeof useAgentDownloads> & {
  /** Only these agents' downloads, when the surface lists fewer agents. */
  agents?: readonly AgentId[]
}) {
  // Retry only where the download itself went wrong. After "busy" or "not
  // confirmed" it may still be running, and a fresh check says what it is.
  const failed =
    outcome?.status === "failed" &&
    !["busy", "not-confirmed", "unavailable"].includes(outcome.reason)
      ? attempted
      : undefined
  const reason = installationOutcomeText(outcome)
  const listed = offers?.filter(
    (offer) => agents === undefined || agents.includes(offer.agent),
  )
  return (
    <section aria-label="Agent downloads" className="flex flex-col">
      {listed ? (
        <>
          <ul role="list" className="m-0 flex flex-col p-0">
            {listed.map((offer) => {
              const name = agentName(offer.agent)
              return (
                <li
                  key={offer.agent}
                  data-agent-download={offer.agent}
                  className="flex min-h-11 list-none items-center gap-3 px-3.5 py-1.5 nessa-text-4"
                >
                  <span className="flex-1 truncate">{name}</span>
                  {offer.installed ? (
                    <span className="nessa-text-3 text-muted-foreground">Installed</span>
                  ) : pending === offer.agent ? (
                    <span
                      role="progressbar"
                      aria-label={`Downloading ${name}`}
                      className="flex items-center gap-1.5 nessa-text-3 text-muted-foreground"
                    >
                      <LoaderCircle
                        aria-hidden="true"
                        className="size-3.5 motion-safe:animate-spin"
                      />
                      Downloading
                    </span>
                  ) : (
                    <button
                      type="button"
                      aria-label={`${failed === offer.agent ? "Retry" : "Download"} ${name} · ${megabytes(offer.archiveBytes)}`}
                      title={
                        failed === offer.agent ? reason : megabytes(offer.archiveBytes)
                      }
                      disabled={pending !== undefined}
                      onClick={() => install(offer.agent)}
                      className={ROW_ACTION}
                    >
                      {failed === offer.agent ? "Retry" : "Download"}
                    </button>
                  )}
                </li>
              )
            })}
          </ul>
          {unavailable ? <CheckAgain unavailable refresh={refresh} /> : null}
        </>
      ) : unavailable || outcome ? (
        <CheckAgain unavailable={unavailable} refresh={refresh} />
      ) : (
        <p className="m-0 flex min-h-11 items-center px-3.5 nessa-text-4 text-muted-foreground">
          Checking…
        </p>
      )}
      {/* Said, not shown: the rows already carry the word. */}
      <p role="status" aria-live="polite" className="sr-only">
        {pending
          ? `Downloading ${agentName(pending)}`
          : unavailable
            ? "Cannot check downloads. Connect to Nessa and try again."
            : reason}
      </p>
    </section>
  )
}

export function AgentDownloads({
  source,
  agents,
  onChanged,
}: {
  source: AgentInstallations
  /** Only these agents' downloads, when the surface lists fewer agents. */
  agents?: readonly AgentId[]
  onChanged?: () => void
}) {
  const state = useAgentDownloads(source, onChanged)
  return <AgentDownloadsView {...state} agents={agents} />
}
