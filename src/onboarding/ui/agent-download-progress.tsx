import { LoaderCircle } from "lucide-react"

/** The installer exposes a final outcome, not received bytes or transfer phases. */
export function AgentDownloadProgress({
  name,
  archiveBytes,
}: {
  name: string
  archiveBytes?: number
}) {
  return (
    <span className="flex min-w-0 flex-col gap-2">
      <span>
        Downloading and verifying {name}. You can leave this view; installation will
        continue.
      </span>
      <span
        role="progressbar"
        aria-label={`Downloading and verifying ${name}`}
        className="inline-flex self-start"
      >
        <LoaderCircle aria-hidden="true" className="size-4 motion-safe:animate-spin" />
      </span>
      {archiveBytes !== undefined ? (
        <span className="nessa-text-2 text-muted-foreground">
          Archive size · {Math.ceil(archiveBytes / 1_000_000)} MB
        </span>
      ) : null}
    </span>
  )
}
