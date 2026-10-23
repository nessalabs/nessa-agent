/**
 * The right panel's parts every shell shares: its toggle, its maximize
 * button, its chord and what it shows while nothing is open. Classic lays it
 * out in its split view (`desktop-app.tsx`); the workspace as a column beside
 * the panes (`workspace/ui/chrome/right-panel.tsx`).
 */
import { EmptyState } from "@nessa-ui/react/empty-state"
import { isMac } from "../adapters/platform"
import { chordLabel, type Chord } from "../model/keyboard"
import { IconButton } from "./icon-button"

/** ⌥⌘B: the left sidebar's ⌘B, for the panel on the other side. */
export const panelChord: Chord = { code: "KeyB", command: true, alt: true }

export function PanelToggle({
  shown,
  controls,
  onToggle,
}: {
  shown: boolean
  /** The id of the panel it shows and hides. */
  controls: string
  onToggle: () => void
}) {
  return (
    <IconButton
      icon="panelRight"
      label={`${shown ? "Hide" : "Show"} Panel`}
      shortcut={chordLabel(panelChord, isMac)}
      aria-expanded={shown}
      aria-controls={controls}
      onClick={onToggle}
    />
  )
}

export function PanelMaximize({
  maximized,
  controls,
  onToggle,
}: {
  maximized: boolean
  controls: string
  onToggle: () => void
}) {
  return (
    <IconButton
      icon={maximized ? "restore" : "maximize"}
      label={maximized ? "Restore Panel" : "Expand Panel"}
      shortcut={maximized ? "Esc" : undefined}
      aria-pressed={maximized}
      aria-controls={controls}
      onClick={onToggle}
    />
  )
}

/** What the panel shows until something is opened in it. */
export function PanelEmpty() {
  return (
    <EmptyState variant="compact" className="desktop-empty-note" title="Nothing open" />
  )
}
