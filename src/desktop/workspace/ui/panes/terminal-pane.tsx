/**
 * A pane showing one of the person's terminals: a slim header (the terminal
 * glyph, its title, a details button and ×) over one real grid. The details
 * popover says where it runs, its folder, its shell and when it started; the
 * session list shows no path, so this is where the path is.
 *
 * A terminal the gateway no longer lists keeps its pane until the person
 * closes it, saying it ended rather than vanishing under the cursor.
 */
import { memo, useCallback, useEffect, useId, useRef, useState } from "react"
import { PopoverSurface } from "@nessa-ui/react/popover-surface"
import { closeTerminal } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectTerminal } from "../../adapters/store/selectors"
import type { PaneFrame } from "../../../split-panes"
import type { PanePlacement } from "../../../split-panes/model/pane-sizing"
import { IconButton } from "../../../ui/icon-button"
import { tooltip } from "../../../ui/tooltip"
import { placeTooltip } from "../../../model/tooltip-placement"
import {
  terminalAgentName,
  terminalAgentOf,
  terminalRunsOnLabel,
  terminalStatusLine,
  type TerminalEntry,
} from "../../model/terminal"
import { TerminalIcon, TerminalStatusIndicator } from "../terminals/terminal-icon"
import { TerminalView } from "../terminals/terminal-view"
import { PaneFocusMark } from "./pane-focus-mark"
import { PaneHeaderFrame } from "./pane-header-frame"
import { usePaneFocus } from "./use-pane-focus"
import "./terminal-pane.css"

export const TerminalPane = memo(function TerminalPane({
  placement,
  frame,
  multi,
  terminalId,
}: {
  placement: PanePlacement
  frame: PaneFrame
  multi: boolean
  terminalId: string
}) {
  const key = placement.key
  const dispatch = useWorkspaceDispatch()
  const focusHandlers = usePaneFocus(key)
  const listed = useWorkspaceSelector((state) => selectTerminal(state, terminalId))
  // The entry last listed, so a terminal that ended keeps its name here.
  const known = useRef(listed)
  if (listed) known.current = listed
  const terminal = known.current
  const title = terminal?.title ?? "Terminal"
  const gone = listed === undefined
  const agent = terminalAgentOf(terminal)
  const status = gone ? undefined : terminal?.programStatus
  const close = useCallback(
    () => void dispatch(closeTerminal(terminalId)),
    [dispatch, terminalId],
  )
  return (
    <article
      className="workspace-pane workspace-terminal-pane"
      {...frame}
      aria-label={title}
      data-terminal-state={gone ? "gone" : terminal?.state}
      data-terminal-agent={agent ?? undefined}
      data-program-state={status?.state}
      {...focusHandlers}
    >
      <PaneFocusMark paneKey={key} multi={multi} />
      <PaneHeaderFrame
        pane={key}
        multi={multi}
        name={
          <>
            <TerminalIcon agent={agent} size={16} />
            <span className="workspace-pane-title workspace-truncate" {...tooltip(title)}>
              {title}
            </span>
            {terminal?.state === "exited" || gone ? (
              <span className="workspace-terminal-ended">
                {endedWords(terminal, gone)}
              </span>
            ) : (
              <TerminalStatusIndicator status={status} />
            )}
          </>
        }
        actions={
          <>
            {terminal ? <TerminalDetails terminal={terminal} /> : null}
            <IconButton
              icon="close"
              label={gone || terminal?.state === "exited" ? "Close" : "End Terminal"}
              draggable={false}
              onClick={close}
            />
          </>
        }
      />
      <div className="workspace-pane-body workspace-terminal-body" data-split-through>
        {gone ? (
          <p className="workspace-terminal-gone">This terminal was closed.</p>
        ) : (
          <TerminalView terminalId={terminalId} label={title} autoFocus />
        )}
      </div>
    </article>
  )
})

function endedWords(terminal: TerminalEntry | undefined, gone: boolean): string {
  if (gone) return "Closed"
  if (terminal?.exitStatus === undefined) return "Ended"
  return `Exited ${terminal.exitStatus}`
}

const started = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
  month: "short",
  day: "numeric",
})

/** Where the terminal runs, its folder, its shell and when it started. */
function TerminalDetails({ terminal }: { terminal: TerminalEntry }) {
  const id = useId()
  const trigger = useRef<HTMLButtonElement>(null)
  const panel = useRef<HTMLDivElement>(null)
  const [open, setOpen] = useState(false)
  const place = useCallback(() => {
    if (!trigger.current || !panel.current) return
    const target = panel.current
    const position = placeTooltip(
      trigger.current.getBoundingClientRect(),
      { width: target.offsetWidth, height: target.offsetHeight },
      { width: window.innerWidth, height: window.innerHeight },
      { prefer: "below", align: "end" },
    )
    target.style.left = `${position.x}px`
    target.style.top = `${position.y}px`
  }, [])
  useEffect(() => {
    if (!open) return
    window.addEventListener("resize", place)
    return () => window.removeEventListener("resize", place)
  }, [open, place])
  return (
    <>
      <IconButton
        ref={trigger}
        icon="details"
        label="Terminal Details"
        draggable={false}
        popoverTarget={id}
        aria-expanded={open}
      />
      <PopoverSurface
        ref={panel}
        id={id}
        popover="auto"
        role="dialog"
        aria-label="Terminal details"
        tabIndex={-1}
        className="workspace-terminal-details"
        onToggle={(event) => {
          const shown = event.newState === "open"
          setOpen(shown)
          if (shown) place()
        }}
      >
        <dl>
          {terminal.programStatus?.app ? (
            <>
              <dt>Program</dt>
              <dd>
                {(() => {
                  const agent = terminalAgentOf(terminal)
                  return agent ? terminalAgentName(agent) : terminal.programStatus.app
                })()}
              </dd>
            </>
          ) : null}
          {terminal.programStatus && terminal.programStatus.state !== "idle" ? (
            <>
              <dt>Status</dt>
              <dd>{terminalStatusLine(terminal.programStatus)}</dd>
            </>
          ) : null}
          <dt>Runs on</dt>
          <dd>{terminalRunsOnLabel(terminal.runsOn)}</dd>
          <dt>Folder</dt>
          <dd className="workspace-terminal-path">{terminal.folder}</dd>
          <dt>Shell</dt>
          <dd className="workspace-terminal-path">{terminal.shell}</dd>
          <dt>Started</dt>
          <dd>{started.format(terminal.startedAtMs)}</dd>
        </dl>
      </PopoverSurface>
    </>
  )
}
