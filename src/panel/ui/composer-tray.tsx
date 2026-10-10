import { AgentDownloads, type AgentInstallations } from "../../onboarding"
import * as React from "react"
import { Check, ChevronLeft, ChevronRight, Plus } from "lucide-react"
import { ChatComposerAction } from "@nessa-ui/react/chat-composer"
import type { ApprovalMode, ApprovalModeChoice } from "../../conversation"

/** The approval row's data: this conversation's mode and the ones its agent honours. */
export interface TrayApproval {
  mode: ApprovalMode
  /** What the gateway says this agent can honour. The tray offers nothing else. */
  modes: readonly ApprovalModeChoice[]
  onChange: (mode: ApprovalMode) => void
  disabled?: boolean
  status?: string
}

/**
 * Where a new conversation's agent runs: this computer, or one of the SSH
 * hosts the gateway names. Offered only before the conversation exists, since
 * it runs there for its whole life.
 */
export interface TrayEnvironment {
  /** The chosen host; absent is this computer. */
  host?: string
  /** What the gateway names under `environments`. The tray offers nothing else. */
  hosts: readonly string[]
  onChange: (host: string | undefined) => void
}

type Page = "root" | "approval" | "confirm-full" | "agents" | "environment"

/** The tray's name for where a conversation runs. */
export function environmentLabel(host: string | undefined): string {
  return host ?? "This computer"
}

/**
 * Arrows move between a page's offered choices, as a radio group's do;
 * choosing stays an explicit Enter or Space, since a choice leaves the page.
 */
function moveBetweenRadios(event: React.KeyboardEvent<HTMLElement>) {
  const step =
    event.key === "ArrowDown" || event.key === "ArrowRight"
      ? 1
      : event.key === "ArrowUp" || event.key === "ArrowLeft"
        ? -1
        : 0
  if (!step && event.key !== "Home" && event.key !== "End") return
  const radios = [
    ...event.currentTarget.querySelectorAll<HTMLButtonElement>(
      "[role=radio]:not(:disabled)",
    ),
  ]
  if (!radios.length) return
  event.preventDefault()
  const at = radios.indexOf(document.activeElement as HTMLButtonElement)
  const next =
    event.key === "Home"
      ? 0
      : event.key === "End"
        ? radios.length - 1
        : (at + step + radios.length) % radios.length
  radios[next]?.focus()
}

export function approvalLabel(mode: ApprovalMode): string {
  switch (mode) {
    case "ask":
      return "Ask"
    case "auto":
      return "Auto"
    case "full":
      return "Full"
  }
}

/**
 * What full access lets the agent do, said before it is turned on. Plain
 * consequences, not a policy: the person decides with these in front of them.
 */
const FULL_ACCESS_RISKS = [
  "Edits and deletes files without asking",
  "Runs any command, online too",
  "You only see it afterwards",
] as const

const FOCUS_RING =
  "outline-none focus-visible:[outline-style:solid] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"

/** The same ring outside a filled button, where an inset ring would sit on the fill. */
const FOCUS_RING_OUTSIDE =
  "outline-none focus-visible:[outline-style:solid] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"

// A light wash rather than the full accent on hover and focus: the ring says
// where focus is, and red text keeps its contrast on the lighter fill.
const ROW = `flex w-full items-center gap-3 rounded-[14px] px-3.5 py-3 text-start nessa-text-4 text-foreground transition-colors hover:bg-accent/70 focus-visible:bg-accent/70 disabled:pointer-events-none disabled:opacity-50 ${FOCUS_RING}`

/**
 * Where the tray sits relative to the + wrapper: its bottom edge just above the
 * composer, its left edge on the composer's, no wider than the composer, and
 * no taller than the panel has room for above it.
 *
 * Measured rather than left to CSS because the composer is not always a box.
 * On a layout compositor the pill composer is `display: contents` (styles.css),
 * so it cannot be the tray's containing block. When the composer has a box and
 * there is room above it for the whole tray, the tray rises above the
 * composer; otherwise — a layout compositor, an expanded or very tall
 * composer — it rises from the composer row, over the draft, and scrolls if the
 * panel is shorter still. The anchor is chosen on every measure, since the
 * composer's box and size change while the tray is open.
 */
function useTrayPlacement(
  open: boolean,
  wrapper: React.RefObject<HTMLDivElement | null>,
  tray: React.RefObject<HTMLDivElement | null>,
) {
  const [placement, setPlacement] = React.useState<React.CSSProperties>()
  React.useLayoutEffect(() => {
    if (!open) return
    const own = wrapper.current
    if (!own) return
    const composer = own.closest<HTMLElement>('[data-slot="pill-composer"]')
    const row = own.closest<HTMLElement>('[data-slot="pill-composer-row"]')
    const panel = own.closest<HTMLElement>("[data-nessa-root]")
    const gap = 10
    const margin = 8
    const measure = () => {
      const top = panel ? panel.getBoundingClientRect().top : 0
      const needed = tray.current?.scrollHeight ?? 0
      const boxed = composer && composer.getClientRects().length > 0 ? composer : null
      const roomAbove = (element: HTMLElement) =>
        element.getBoundingClientRect().top - top - gap - margin
      const anchor =
        boxed && (!row || roomAbove(boxed) >= needed) ? boxed : (row ?? boxed)
      if (!anchor) return
      const from = own.getBoundingClientRect()
      const to = anchor.getBoundingClientRect()
      const next = {
        bottom: from.bottom - to.top + gap,
        left: to.left - from.left,
        width: Math.min(336, to.width),
        maxHeight: Math.max(120, roomAbove(anchor)),
      }
      setPlacement((previous) =>
        previous &&
        previous.bottom === next.bottom &&
        previous.left === next.left &&
        previous.width === next.width &&
        previous.maxHeight === next.maxHeight
          ? previous
          : next,
      )
    }
    measure()
    if (typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    for (const element of [own, composer, row, tray.current]) {
      if (element) observer.observe(element)
    }
    return () => observer.disconnect()
  }, [open, wrapper, tray])
  return placement
}

/**
 * The composer's +: a small tray that rises above the composer.
 *
 * Each entry is one row. A row with choices of its own — tool approval — takes
 * the tray over in place, with a way back, rather than opening a menu beside
 * it: the panel is narrow, and a side menu would open off its edge.
 *
 * It is a non-modal popover: Escape steps back a page and then closes it,
 * a press outside closes it, and so does focus leaving it, so the keyboard
 * never works behind a tray that stays open. Every page puts focus where the
 * person was — the checked mode, the row they came back to — and every close
 * that was asked for returns focus to +.
 */
export function ComposerTray({
  disabled,
  onChoose,
  linksFiles = true,
  onSignOut,
  approval,
  environment,
  agentInstallations,
}: {
  /** Files cannot be added while earlier ones are still being read. */
  disabled: boolean
  onChoose: () => void
  /**
   * Whether a file that is not an image can be added, linked by its path on
   * this machine. Not for a conversation on an SSH host: it is offered images
   * alone, which carry their bytes.
   */
  linksFiles?: boolean
  onSignOut?: () => void
  approval?: TrayApproval
  /** Absent when the gateway names no host, or the conversation exists. */
  environment?: TrayEnvironment
  agentInstallations?: AgentInstallations
}) {
  const [open, setOpen] = React.useState(false)
  const [page, setPage] = React.useState<Page>("root")
  // What to focus when a page shows; absent means the page's first control.
  const [focusKey, setFocusKey] = React.useState<string>()
  const root = React.useRef<HTMLDivElement>(null)
  const trigger = React.useRef<HTMLButtonElement>(null)
  const tray = React.useRef<HTMLDivElement>(null)
  const trayId = React.useId()
  const placement = useTrayPlacement(open, root, tray)
  // A page that needs approval falls back to the first page if approval goes
  // away while it shows, rather than leaving an empty tray.
  const shown: Page =
    page === "agents" && agentInstallations
      ? "agents"
      : page === "environment"
        ? environment && environment.hosts.length > 0
          ? "environment"
          : "root"
        : approval && approval.modes.length > 1
          ? page
          : "root"

  const go = React.useCallback((next: Page, focus?: string) => {
    setPage(next)
    setFocusKey(focus)
  }, [])

  const close = React.useCallback((returnFocus: boolean) => {
    setOpen(false)
    setPage("root")
    setFocusKey(undefined)
    if (returnFocus) trigger.current?.focus()
  }, [])

  React.useEffect(() => {
    if (!open) return
    const target =
      (focusKey &&
        tray.current?.querySelector<HTMLElement>(`[data-tray-focus="${focusKey}"]`)) ||
      tray.current?.querySelector<HTMLElement>("button:not(:disabled)")
    target?.focus()
  }, [open, shown, focusKey])

  React.useEffect(() => {
    if (!open) return
    const away = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) close(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      if (shown === "confirm-full") go("approval", "mode-full")
      else if (shown === "agents") go("root", "agents-row")
      else if (shown === "environment") go("root", "environment-row")
      else if (shown === "approval") go("root", "approval-row")
      else close(true)
    }
    document.addEventListener("pointerdown", away)
    document.addEventListener("keydown", escape)
    return () => {
      document.removeEventListener("pointerdown", away)
      document.removeEventListener("keydown", escape)
    }
  }, [open, shown, close, go])

  const label =
    onSignOut || approval || environment || agentInstallations
      ? "More options"
      : "Add attachment"
  return (
    <div
      ref={root}
      className="relative flex shrink-0"
      onBlur={(event) => {
        // Focus moving to something else on the page closes the tray. Focus
        // lost to nothing — a page swap unmounting the focused row — does not.
        const next = event.relatedTarget as Node | null
        if (open && next && !root.current?.contains(next)) close(false)
      }}
    >
      <ChatComposerAction
        ref={trigger}
        className="nessa-composer-control"
        aria-label={label}
        title={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? trayId : undefined}
        disabled={
          disabled && !onSignOut && !approval && !environment && !agentInstallations
        }
        onClick={() => (open ? close(false) : setOpen(true))}
      >
        <Plus
          aria-hidden="true"
          data-slot="composer-tray-plus"
          className="transition-transform duration-200 motion-reduce:transition-none"
          style={{ transform: open ? "rotate(45deg)" : undefined }}
        />
      </ChatComposerAction>
      {open ? (
        <div
          ref={tray}
          id={trayId}
          role="dialog"
          aria-label="Composer options"
          style={placement}
          className="nessa-composer-tray absolute bottom-[calc(100%+10px)] left-0 z-50 w-[21rem] overflow-y-auto overscroll-contain rounded-[22px] border border-border/60 bg-popover p-1.5 text-popover-foreground shadow-xl"
        >
          {shown === "root" ? (
            <div key="root" className="nessa-composer-tray-page flex flex-col">
              <button
                type="button"
                className={ROW}
                disabled={disabled}
                onClick={() => {
                  // Focus goes back to + first, so it is where the file dialog
                  // returns it.
                  close(true)
                  onChoose()
                }}
              >
                <span className="flex-1">{linksFiles ? "Add files" : "Add images"}</span>
              </button>
              {approval && approval.modes.length > 1 ? (
                <button
                  type="button"
                  data-tray-focus="approval-row"
                  className={ROW}
                  disabled={approval.disabled}
                  onClick={() => go("approval", `mode-${approval.mode}`)}
                >
                  <span className="flex flex-1 flex-col">
                    <span>Tool approval</span>
                    {approval.status ? (
                      <span className="nessa-text-2 text-muted-foreground">
                        {approval.status}
                      </span>
                    ) : null}
                  </span>
                  {/* Full access stays red wherever it is named. */}
                  <span
                    className={
                      approval.mode === "full"
                        ? "text-destructive"
                        : "text-muted-foreground"
                    }
                  >
                    {approvalLabel(approval.mode)}
                  </span>
                  <ChevronRight
                    aria-hidden="true"
                    className="size-4 text-muted-foreground"
                  />
                </button>
              ) : null}
              {environment && environment.hosts.length > 0 ? (
                <button
                  type="button"
                  data-tray-focus="environment-row"
                  className={ROW}
                  onClick={() =>
                    go(
                      "environment",
                      `host-${environment.hosts.indexOf(environment.host ?? "") + 1}`,
                    )
                  }
                >
                  <span className="flex-1">Run on</span>
                  <span className="truncate text-muted-foreground">
                    {environmentLabel(environment.host)}
                  </span>
                  <ChevronRight
                    aria-hidden="true"
                    className="size-4 shrink-0 text-muted-foreground"
                  />
                </button>
              ) : null}
              {agentInstallations ? (
                <button
                  type="button"
                  data-tray-focus="agents-row"
                  className={ROW}
                  onClick={() => go("agents")}
                >
                  <span className="flex-1">Agent downloads</span>
                  <ChevronRight aria-hidden="true" className="size-4" />
                </button>
              ) : null}
              {onSignOut ? (
                <button
                  type="button"
                  className={ROW}
                  onClick={() => {
                    close(true)
                    onSignOut()
                  }}
                >
                  <span className="flex-1">Sign out</span>
                </button>
              ) : null}
            </div>
          ) : shown === "agents" && agentInstallations ? (
            <div
              key="agents"
              className="nessa-composer-tray-page flex flex-col gap-3 p-2"
            >
              <button
                type="button"
                className={ROW}
                onClick={() => go("root", "agents-row")}
              >
                <ChevronLeft aria-hidden="true" className="size-4" />
                Back
              </button>
              <AgentDownloads source={agentInstallations} />
            </div>
          ) : shown === "environment" && environment ? (
            <div key="environment" className="nessa-composer-tray-page flex flex-col">
              <button
                type="button"
                aria-label="Back from Run on"
                onClick={() => go("root", "environment-row")}
                className={`flex items-center gap-1 self-start rounded-[12px] px-2 py-2 nessa-text-3 text-muted-foreground transition-colors hover:text-foreground focus-visible:text-foreground ${FOCUS_RING}`}
              >
                <ChevronLeft aria-hidden="true" className="size-4" />
                Run on
              </button>
              <div
                role="radiogroup"
                aria-label="Run on"
                className="flex flex-col"
                onKeyDown={moveBetweenRadios}
              >
                {/* This computer first, as position 0; each host after it. */}
                {[undefined, ...environment.hosts].map((host, index) => {
                  const chosen = environment.host === host
                  return (
                    <button
                      key={host ?? ""}
                      type="button"
                      role="radio"
                      aria-checked={chosen}
                      data-tray-focus={`host-${index}`}
                      tabIndex={chosen ? 0 : -1}
                      className={ROW}
                      onClick={() => {
                        go("root", "environment-row")
                        if (!chosen) environment.onChange(host)
                      }}
                    >
                      <span className="flex flex-1 flex-col">
                        <span className="break-all">{environmentLabel(host)}</span>
                        {host ? (
                          <span className="nessa-text-2 text-muted-foreground">SSH</span>
                        ) : null}
                      </span>
                      {chosen ? (
                        <Check aria-hidden="true" className="size-4 text-foreground" />
                      ) : null}
                    </button>
                  )
                })}
              </div>
            </div>
          ) : shown === "approval" && approval ? (
            <div key="approval" className="nessa-composer-tray-page flex flex-col">
              <button
                type="button"
                aria-label="Back from Tool approval"
                onClick={() => go("root", "approval-row")}
                className={`flex items-center gap-1 self-start rounded-[12px] px-2 py-2 nessa-text-3 text-muted-foreground transition-colors hover:text-foreground focus-visible:text-foreground ${FOCUS_RING}`}
              >
                <ChevronLeft aria-hidden="true" className="size-4" />
                Tool approval
              </button>
              <div
                role="radiogroup"
                aria-label="Tool approval"
                className="flex flex-col"
                onKeyDown={moveBetweenRadios}
              >
                {approval.modes.map((choice) => {
                  const chosen = approval.mode === choice.id
                  return (
                    <button
                      key={choice.id}
                      type="button"
                      role="radio"
                      aria-checked={chosen}
                      aria-labelledby={`${trayId}-${choice.id}-name`}
                      data-tray-focus={`mode-${choice.id}`}
                      // One stop in the tab order: the checked mode.
                      tabIndex={chosen ? 0 : -1}
                      disabled={approval.disabled}
                      className={ROW}
                      onClick={() => {
                        // Turning full access on asks first; every other
                        // choice, including leaving full access, does not.
                        if (choice.id === "full" && !chosen) {
                          go("confirm-full", "cancel")
                          return
                        }
                        go("root", "approval-row")
                        if (!chosen) approval.onChange(choice.id)
                      }}
                    >
                      <span
                        id={`${trayId}-${choice.id}-name`}
                        className={`flex-1 ${choice.id === "full" ? "text-destructive" : ""}`}
                      >
                        {approvalLabel(choice.id)}
                      </span>
                      {chosen ? (
                        <Check aria-hidden="true" className="size-4 text-foreground" />
                      ) : null}
                    </button>
                  )
                })}
              </div>
            </div>
          ) : shown === "confirm-full" && approval ? (
            <div
              key="confirm-full"
              role="alertdialog"
              aria-labelledby={`${trayId}-full-title`}
              aria-describedby={`${trayId}-full-risks`}
              className="nessa-composer-tray-page flex flex-col gap-3 p-2"
            >
              <p
                id={`${trayId}-full-title`}
                className="m-0 px-1.5 pt-1 nessa-text-5 font-semibold text-foreground"
              >
                Turn on full access?
              </p>
              {/* Tinted red, so the card says "risk" before a word is read.
                  `role="list"` keeps it a list to WebKit, which drops the role
                  from a list styled without markers. */}
              <ul
                id={`${trayId}-full-risks`}
                role="list"
                className="m-0 flex flex-col gap-2 rounded-[16px] border border-destructive/25 bg-destructive/10 px-4 py-3 nessa-text-3 text-foreground"
              >
                {FULL_ACCESS_RISKS.map((risk) => (
                  <li key={risk} className="flex list-none items-center gap-2.5">
                    <span
                      aria-hidden="true"
                      className="size-1.5 shrink-0 rounded-full bg-destructive"
                    />
                    {risk}
                  </li>
                ))}
              </ul>
              <div className="flex gap-2">
                {/* First, so it is where focus lands: Enter backs out. */}
                <button
                  type="button"
                  data-tray-focus="cancel"
                  onClick={() => go("approval", "mode-full")}
                  className={`flex-1 whitespace-nowrap rounded-full bg-muted px-4 py-2.5 nessa-text-4 font-medium text-foreground transition-colors hover:bg-accent focus-visible:bg-accent ${FOCUS_RING_OUTSIDE}`}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => {
                    go("root", "approval-row")
                    approval.onChange("full")
                  }}
                  className={`flex-1 whitespace-nowrap rounded-full bg-destructive px-4 py-2.5 nessa-text-4 font-medium text-destructive-foreground transition-opacity hover:opacity-90 ${FOCUS_RING_OUTSIDE}`}
                >
                  Turn on
                </button>
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
