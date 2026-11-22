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
  scrollsHistory,
}: {
  terminalId: string
  label: string
  autoFocus?: boolean
  className?: string
  onCopyText?: ComponentProps<typeof Terminal>["onCopyText"]
  scrollsHistory?: ComponentProps<typeof Terminal>["scrollsHistory"]
}) {
  const source = useTerminalSource()
  // Made in an effect, so a view the page let go of is released, and one
  // mounted twice in development is two views, never one released twice.
  const [held, setHeld] = useState<{ terminalId: string; view: SourceView } | null>(null)
  useEffect(() => {
    if (!source) return
    const made = { terminalId, view: source.view(terminalId) }
    setHeld(made)
    return () => {
      made.view.release()
      setHeld((current) => (current === made ? null : current))
    }
  }, [source, terminalId])
  // A view of another terminal, held until the effect swaps it, is never drawn.
  const view = held?.terminalId === terminalId ? held.view : null
  const subscribe = useMemo(
    () => (listener: (message: TerminalMessage) => void) =>
      view
        ? view.subscribe((message) => listener(message as unknown as TerminalMessage))
        : () => {},
    [view],
  )
  if (!view) return null
  // One Terminal per view: a pane switched to another terminal mounts a
  // fresh one, whose first measured fit attaches the new view. Kept, the
  // old one's fit would be unchanged, so the new view would never attach.
  return (
    <Terminal
      key={terminalId}
      aria-label={label}
      className={className}
      autoFocus={autoFocus}
      subscribe={subscribe}
      onInput={view.input}
      onFit={view.fit}
      onFocusView={view.focus}
      onHistory={view.history}
      onCopyText={onCopyText}
      scrollsHistory={scrollsHistory}
    />
  )
})
