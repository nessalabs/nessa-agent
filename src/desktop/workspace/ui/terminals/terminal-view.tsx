/**
 * One view of a terminal, drawn by the design system's `Terminal` over a
 * view the window's terminal source holds for as long as this is mounted.
 * A pane and a Run block's output both draw it.
 */
import { memo, useEffect, useMemo, useState, type ComponentProps } from "react"
import type { TerminalView as SourceView } from "../../application/ports"
import { Terminal, type TerminalMessage } from "@nessa-ui/react/terminal"
import { useTerminalSource } from "./terminal-source"

export const TerminalView = memo(function TerminalView({
  terminalId,
  label,
  autoFocus,
  className,
  onCopyText,
}: {
  terminalId: string
  label: string
  autoFocus?: boolean
  className?: string
  onCopyText?: ComponentProps<typeof Terminal>["onCopyText"]
}) {
  const source = useTerminalSource()
  // Made in an effect, so a view the page let go of is released, and one
  // mounted twice in development is two views, never one released twice.
  const [view, setView] = useState<SourceView | null>(null)
  useEffect(() => {
    if (!source) return
    const made = source.view(terminalId)
    setView(made)
    return () => {
      made.release()
      setView((current) => (current === made ? null : current))
    }
  }, [source, terminalId])
  const subscribe = useMemo(
    () => (listener: (message: TerminalMessage) => void) =>
      view
        ? view.subscribe((message) => listener(message as unknown as TerminalMessage))
        : () => {},
    [view],
  )
  if (!view) return null
  return (
    <Terminal
      aria-label={label}
      className={className}
      autoFocus={autoFocus}
      subscribe={subscribe}
      onInput={view.input}
      onFit={view.fit}
      onFocusView={view.focus}
      onHistory={view.history}
      onCopyText={onCopyText}
    />
  )
})
