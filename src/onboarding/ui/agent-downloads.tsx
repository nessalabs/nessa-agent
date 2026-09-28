import type {
  AgentInstallations,
  InstallationOutcome,
} from "../application/agent-installations"
import { AGENT_CHOICES } from "../model/onboarding"
import { useAgentDownloads } from "./use-agent-downloads"

function outcomeText(outcome: InstallationOutcome | undefined): string | undefined {
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

export function AgentDownloadsView({
  offers,
  pending,
  outcome,
  unavailable,
  refresh,
  install,
}: ReturnType<typeof useAgentDownloads>) {
  return (
    <section aria-label="Agent downloads" className="flex flex-col gap-2">
      <p className="nessa-text-2 text-muted-foreground">
        Download only the agents you use. Your own account is still required.
      </p>
      {offers?.map((offer) => (
        <div
          key={offer.agent}
          className="flex items-center justify-between gap-2 nessa-text-2"
        >
          <span>{AGENT_CHOICES.find((choice) => choice.id === offer.agent)?.name}</span>
          {offer.installed ? (
            <span>Installed</span>
          ) : (
            <button
              type="button"
              aria-label={`Download ${AGENT_CHOICES.find((choice) => choice.id === offer.agent)?.name} · ${Math.ceil(offer.archiveBytes / 1_000_000)} MB`}
              disabled={pending !== undefined}
              onClick={() => install(offer.agent)}
              className="rounded-full border border-border px-3 py-2 focus-visible:outline focus-visible:outline-ring disabled:opacity-60"
            >
              {pending === offer.agent
                ? "Downloading…"
                : `Download · ${Math.ceil(offer.archiveBytes / 1_000_000)} MB`}
            </button>
          )}
        </div>
      ))}
      <p role="status" aria-live="polite" className="nessa-text-2 text-muted-foreground">
        {pending
          ? "Downloading and verifying the runtime. You can leave this view; installation will continue."
          : unavailable
            ? "Cannot check downloads. Connect to Nessa and try again."
            : outcomeText(outcome)}
      </p>
      <button
        type="button"
        onClick={refresh}
        className="self-start rounded-full border border-border px-3 py-2 nessa-text-2 focus-visible:outline focus-visible:outline-ring"
      >
        Check downloads
      </button>
    </section>
  )
}

export function AgentDownloads({
  source,
  onInstalled,
}: {
  source: AgentInstallations
  onInstalled?: () => void
}) {
  const state = useAgentDownloads(source, onInstalled)
  return <AgentDownloadsView {...state} />
}
