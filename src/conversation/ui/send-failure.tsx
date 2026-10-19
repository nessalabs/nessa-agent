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

/** A short send failure; technical details remain available through Copy. */
export function SendFailure({
  details,
  code,
  onRetry,
  onDiscard,
  pending = false,
}: {
  details: string
  code?: string
  onRetry?: () => void
  onDiscard?: () => void
  pending?: boolean
}) {
  const write = useContext(Clipboard)
  const [copied, setCopied] = useState<string | null>(null)
  const [copyFailed, setCopyFailed] = useState(false)
  const diagnostic = code ? `${code}\n${details}` : details
  return (
    <div
      role="status"
      className="flex flex-wrap items-center gap-2 mb-2 text-xs text-muted-foreground"
      data-send-failure
    >
      <span>Error sending{code ? ` · ${code}` : ""}</span>
      <button
        type="button"
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
      {onRetry ? (
        <button type="button" disabled={pending} onClick={onRetry}>
          Send Again
        </button>
      ) : null}
      {onDiscard ? (
        <button type="button" onClick={onDiscard}>
          Discard
        </button>
      ) : null}
    </div>
  )
}
