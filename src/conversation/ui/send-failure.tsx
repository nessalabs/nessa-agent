import { createContext, useContext, useState, type ReactNode } from "react"
import { Check, Copy } from "lucide-react"

const Clipboard = createContext<((text: string) => Promise<void>) | null>(null)

/** The host supplies clipboard access; an unavailable or failed write is not success. */
export function ClipboardProvider({
  write,
  children,
}: {
  write: (text: string) => Promise<void>
  children: ReactNode
}) {
  return <Clipboard.Provider value={write}>{children}</Clipboard.Provider>
}

/** The host's clipboard writer, or null where it supplies none. */
export function useClipboardWrite(): ((text: string) => Promise<void>) | null {
  return useContext(Clipboard)
}

/**
 * A refused send, as a small card above the composer: what happened over a
 * short why (`summary`, else the code), then Copy (the code and the full
 * details, which may be long, for a report), Discard and Send Again.
 * It is a card, not a line, so it reads as the message that did not go, and
 * the composer below it stays free of anything said.
 */
export function SendFailure({
  details,
  code,
  summary,
  onRetry,
  onDiscard,
  pending = false,
}: {
  details: string
  code?: string
  /** A few plain words of why; the details stay behind Copy. */
  summary?: string
  onRetry?: () => void
  onDiscard?: () => void
  pending?: boolean
}) {
  const write = useContext(Clipboard)
  const [copied, setCopied] = useState<string | null>(null)
  const [copyFailed, setCopyFailed] = useState(false)
  const diagnostic = code ? `${code}\n${details}` : details
  const quiet =
    "inline-flex min-h-7 shrink-0 cursor-pointer items-center rounded-full border-0 bg-transparent px-2.5 font-sans nessa-text-2 font-medium text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground disabled:cursor-default disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
  return (
    <div
      role="status"
      className="mb-2 flex min-w-0 items-center gap-2 rounded-xl bg-popover/90 py-2 ps-3 pe-2 font-sans text-popover-foreground shadow-sm ring-1 ring-border/50 ring-inset animate-in fade-in-0 slide-in-from-bottom-1 motion-reduce:animate-none"
      data-send-failure
    >
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="nessa-text-1 text-muted-foreground">Error sending</span>
        {summary || code ? (
          <span className="truncate nessa-text-2">{summary ?? code}</span>
        ) : null}
      </span>
      <button
        type="button"
        className={`${quiet} px-2`}
        aria-label={copyFailed ? "Copy failed; try again" : "Copy error details"}
        title={copyFailed ? "Copy failed; try again" : "Copy error details"}
        disabled={!write}
        onClick={async () => {
          if (!write) return
          try {
            await write(diagnostic)
            setCopied(diagnostic)
            setCopyFailed(false)
          } catch {
            setCopyFailed(true)
          }
        }}
      >
        {copied === diagnostic && !copyFailed ? <Check size={14} /> : <Copy size={14} />}
      </button>
      {onDiscard ? (
        <button type="button" className={quiet} onClick={onDiscard}>
          Discard
        </button>
      ) : null}
      {onRetry ? (
        <button
          type="button"
          className="inline-flex min-h-7 shrink-0 cursor-pointer items-center rounded-full border-0 bg-accent px-3 font-sans nessa-text-2 font-semibold text-(--nessa-chat-accent-ink) transition-colors hover:bg-accent/70 disabled:cursor-default disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          disabled={pending}
          onClick={onRetry}
        >
          Send Again
        </button>
      ) : null}
    </div>
  )
}
