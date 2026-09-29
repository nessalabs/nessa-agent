import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react"
import { DesktopIcon } from "../../../ui/icons"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannels, selectListedSessions } from "../../adapters/store/selectors"
import { useNow } from "../../adapters/dom/clock"
import { focusComposer } from "../../adapters/dom/focus"
import { isMac } from "../../../adapters/platform"
import { commandKey, commandLabel } from "../../../model/keyboard"
import { agentName, agentOf } from "../../model/workspace-index"
import { switcherRows, type SwitcherRow } from "../../model/session-search"
import { sessionTime } from "../../model/time-labels"
import { StatusGlyph } from "../chrome/status-glyph"
import { useWorkspaceFrame } from "../workspace-frame"
import "./quick-switcher.css"

export type SwitcherMode = "open" | "split"

/** Characters of `text` at `hits` drawn brighter, as the search matched them. */
function Highlight({ text, hits }: { text: string; hits: readonly number[] }) {
  if (hits.length === 0) return <>{text}</>
  const matched = new Set(hits)
  return (
    <>
      {[...text].map((char, index) =>
        matched.has(index) ? (
          <mark key={index} className="workspace-hit">
            {char}
          </mark>
        ) : (
          <Fragment key={index}>{char}</Fragment>
        ),
      )}
    </>
  )
}

/**
 * Jump to a session or channel, or start one, by typing (⌘K); in `split`
 * mode, what is picked opens beside the focused pane. The rows and their
 * order are the model's (`model/session-search.ts`); this only draws them
 * and moves through them. Modal: focus stays in the field.
 */
/**
 * A pick hands the caret to the focused pane's composer, which may still be
 * filling in (`focusComposer` tries for a few frames). The try is kept so it
 * can be called off — by the switcher opening again, or the person pressing
 * somewhere else first — rather than land the caret after they moved on.
 */
let stopHanding: () => void = () => {}

function handToComposer() {
  stopHanding()
  const finish = () => {
    window.removeEventListener("pointerdown", callOff, true)
    stopHanding = () => {}
  }
  // Landed (or given up), there is nothing left to call off: the listener goes.
  const stop = focusComposer(document, finish)
  const callOff = () => {
    stop()
    finish()
  }
  window.addEventListener("pointerdown", callOff, true)
  stopHanding = callOff
}

export function QuickSwitcher({
  mode,
  channelId,
  onClose,
  onPick,
}: {
  mode: SwitcherMode
  /** The channel a new session would start in. */
  channelId: string
  onClose: () => void
  /** `beside` when the command key was held, or the switcher opened to split. */
  onPick: (row: SwitcherRow, beside: boolean) => void
}) {
  const sessions = useWorkspaceSelector(selectListedSessions)
  const channels = useWorkspaceSelector(selectChannels)
  const [query, setQuery] = useState("")
  const [active, setActive] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const dialogRef = useRef<HTMLDivElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const now = useNow(60_000)
  const newShortcut = useWorkspaceFrame().shortcut("newSession")
  const channelName = (id: string) =>
    channels.find((channel) => channel.id === id)?.name ?? ""

  // Modal: focus stays in the field, even when a menu that just closed hands
  // focus back to its trigger a frame after the switcher opened. Closed
  // without a pick, focus goes back where it was; a pick hands it to the
  // focused pane's composer, wherever the pick put it.
  const picked = useRef(false)
  useEffect(() => {
    // A pick's caret still on its way from the last time — a new pane filling
    // in — is called off: the switcher has the keyboard now.
    stopHanding()
    const restore = document.activeElement as HTMLElement | null
    inputRef.current?.focus()
    const keep = (event: FocusEvent) => {
      if (!dialogRef.current?.contains(event.target as Node)) inputRef.current?.focus()
    }
    document.addEventListener("focusin", keep)
    return () => {
      document.removeEventListener("focusin", keep)
      if (picked.current) handToComposer()
      else restore?.focus?.()
    }
  }, [])
  const choose = (row: SwitcherRow, beside: boolean) => {
    picked.current = true
    onPick(row, beside)
  }

  const rows = useMemo(
    () => switcherRows({ query, sessions, channels, channelId }),
    [query, sessions, channels, channelId],
  )
  const clamped = Math.min(active, rows.length - 1)

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${clamped}"]`)
      ?.scrollIntoView({ block: "nearest" })
  }, [clamped])

  const onKeyDown = (event: ReactKeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault()
      onClose()
    } else if (event.key === "ArrowDown" || (event.ctrlKey && event.key === "n")) {
      event.preventDefault()
      setActive((clamped + 1) % rows.length)
    } else if (event.key === "ArrowUp" || (event.ctrlKey && event.key === "p")) {
      event.preventDefault()
      setActive((clamped - 1 + rows.length) % rows.length)
    } else if (event.key === "Enter") {
      event.preventDefault()
      const row = rows[clamped]
      if (row) choose(row, mode === "split" || commandKey(event, isMac))
    } else if (event.key === "Tab") {
      event.preventDefault()
    }
  }

  let lastGroup: string | null = null
  return (
    <div
      className="workspace-overlay"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <div
        ref={dialogRef}
        className="workspace-switcher"
        role="dialog"
        aria-modal="true"
        aria-label={mode === "split" ? "Open beside" : "Jump to"}
        onKeyDown={onKeyDown}
      >
        <div className="workspace-switcher-field">
          <DesktopIcon name={mode === "split" ? "splitRight" : "search"} />
          <input
            ref={inputRef}
            role="combobox"
            aria-expanded="true"
            aria-controls="workspace-switcher-results"
            aria-activedescendant={
              rows[clamped] ? `workspace-result-${clamped}` : undefined
            }
            aria-autocomplete="list"
            placeholder={
              mode === "split"
                ? "Open beside the current pane…"
                : "Jump to a session or channel…"
            }
            value={query}
            spellCheck={false}
            onChange={(event) => {
              setQuery(event.target.value)
              setActive(0)
            }}
          />
        </div>
        <div
          className="workspace-results"
          id="workspace-switcher-results"
          role="listbox"
          ref={listRef}
        >
          {rows.map((row, index) => {
            const heading = row.group && row.group !== lastGroup ? row.group : null
            lastGroup = row.group
            return (
              <Fragment key={rowKey(row)}>
                {heading ? (
                  <div className="workspace-results-group" role="presentation">
                    {heading}
                  </div>
                ) : null}
                <div
                  id={`workspace-result-${index}`}
                  role="option"
                  aria-selected={index === clamped}
                  data-index={index}
                  className="workspace-result"
                  onPointerMove={() => index !== clamped && setActive(index)}
                  onClick={(event) =>
                    choose(row, mode === "split" || commandKey(event, isMac))
                  }
                >
                  {index === clamped ? <RowKeys mode={mode} /> : null}
                  {row.kind === "session" ? (
                    <>
                      <StatusGlyph status={row.session.status} idle />
                      <span className="workspace-result-title">
                        <Highlight text={row.session.title} hits={row.hits} />
                      </span>
                      <span className="workspace-result-meta">
                        #{channelName(row.session.channelId)} ·{" "}
                        {agentName(agentOf(row.session.model))}
                      </span>
                      <span className="workspace-result-trail">
                        {sessionTime(row.session.updatedAt, now)}
                      </span>
                    </>
                  ) : row.kind === "channel" ? (
                    <>
                      <span className="workspace-result-icon">
                        <DesktopIcon
                          name={row.channel.private ? "privateChannel" : "channel"}
                        />
                      </span>
                      <span className="workspace-result-title">
                        <Highlight text={row.channel.name} hits={row.hits} />
                      </span>
                      <span className="workspace-result-meta">{row.channel.topic}</span>
                    </>
                  ) : row.text ? (
                    <>
                      <span className="workspace-result-icon">
                        <DesktopIcon name="enter" />
                      </span>
                      <span className="workspace-result-title">“{row.text}”</span>
                      <span className="workspace-result-meta">
                        new session in #{channelName(row.channelId)}
                      </span>
                    </>
                  ) : (
                    <>
                      <span className="workspace-result-icon">
                        <DesktopIcon name="newSession" />
                      </span>
                      <span className="workspace-result-title">
                        New session in #{channelName(row.channelId)}
                      </span>
                      {newShortcut ? (
                        <span className="workspace-result-trail">
                          <kbd className="workspace-kbd">{newShortcut}</kbd>
                        </span>
                      ) : null}
                    </>
                  )}
                </div>
              </Fragment>
            )
          })}
        </div>
      </div>
    </div>
  )
}

/**
 * What ↩ does on the chosen row, said on the row itself rather than in a bar
 * of hints: open it — beside, when the switcher opened to put something
 * there — and, jumping, ⌘↩ to open it beside instead.
 */
function RowKeys({ mode }: { mode: SwitcherMode }) {
  return (
    <span className="workspace-result-keys" aria-hidden="true">
      {mode === "split" ? (
        <>
          Open Beside <kbd>↩</kbd>
        </>
      ) : (
        <>
          Open <kbd>↩</kbd>
          <span aria-hidden="true">·</span>
          Beside <kbd>{commandLabel(isMac)}↩</kbd>
        </>
      )}
    </span>
  )
}

function rowKey(row: SwitcherRow): string {
  if (row.kind === "session") return `session-${row.session.id}`
  if (row.kind === "channel") return `channel-${row.channel.id}`
  return row.text ? "ask" : "new"
}
