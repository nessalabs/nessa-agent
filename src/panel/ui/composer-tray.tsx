import * as React from "react"
import { Check, ChevronLeft, ChevronRight, Plus } from "lucide-react"
import { ChatComposerAction } from "@nessa-ui/react/chat-composer"
import { APPROVAL_MODES, APPROVAL_MODE_TEXT, type ApprovalMode } from "../../conversation"

/** The approval row's data: this conversation's mode and the ones its agent honours. */
export interface TrayApproval {
  mode: ApprovalMode
  /** What the gateway says this agent can honour. The tray offers nothing else. */
  modes: readonly ApprovalMode[]
  onChange: (mode: ApprovalMode) => void
}

type Page = "root" | "approval" | "confirm-full"

/**
 * What full access lets the agent do, said before it is turned on. Plain
 * consequences, not a policy: the person decides with these in front of them.
 */
const FULL_ACCESS_RISKS = [
  "Edits and deletes files without asking",
  "Runs any command, online too",
  "You only see it afterwards",
] as const

const ROW =
  "flex w-full items-center gap-3 rounded-[14px] px-3.5 py-3 text-start nessa-text-4 text-foreground outline-none transition-colors hover:bg-accent focus-visible:bg-accent disabled:pointer-events-none disabled:opacity-50"

/**
 * The composer's +: a small tray that rises above the composer.
 *
 * Each entry is one row. A row with choices of its own — tool approval — takes
 * the tray over in place, with a way back, rather than opening a menu beside
 * it: the panel is narrow, and a side menu would open off its edge.
 *
 * The tray is positioned from the + button's own wrapper, which has a box on
 * every compositor, and measured against the composer so it rises clear of it
 * (see `useTrayPlacement`).
 */
/**
 * Where the tray sits relative to the + wrapper: its bottom edge just above the
 * composer, its left edge on the composer's, no wider than the composer.
 *
 * Measured rather than left to CSS because the composer is not always a box.
 * On a layout compositor the pill composer is `display: contents` (styles.css),
 * so it cannot be the tray's containing block, and a tray anchored to it would
 * resolve against the panel instead. The composer row keeps its box there, so
 * the composer is measured when it has one and the row when it does not. The
 * observer follows the draft growing and attachments arriving while it is open.
 */
function useTrayPlacement(
  open: boolean,
  wrapper: React.RefObject<HTMLDivElement | null>,
) {
  const [placement, setPlacement] = React.useState<React.CSSProperties>()
  React.useLayoutEffect(() => {
    if (!open) return
    const own = wrapper.current
    const composer = own?.closest<HTMLElement>('[data-slot="pill-composer"]')
    const anchor =
      composer && composer.getClientRects().length > 0
        ? composer
        : own?.closest<HTMLElement>('[data-slot="pill-composer-row"]')
    if (!own || !anchor) return
    const measure = () => {
      const from = own.getBoundingClientRect()
      const to = anchor.getBoundingClientRect()
      setPlacement({
        bottom: from.bottom - to.top + 10,
        left: to.left - from.left,
        width: Math.min(336, to.width),
      })
    }
    measure()
    if (typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    observer.observe(anchor)
    observer.observe(own)
    return () => observer.disconnect()
  }, [open, wrapper])
  return placement
}

export function ComposerTray({
  disabled,
  onChoose,
  onSignOut,
  approval,
}: {
  /** Files cannot be added while earlier ones are still being read. */
  disabled: boolean
  onChoose: () => void
  onSignOut?: () => void
  approval?: TrayApproval
}) {
  const [open, setOpen] = React.useState(false)
  const [page, setPage] = React.useState<Page>("root")
  const root = React.useRef<HTMLDivElement>(null)
  const trigger = React.useRef<HTMLButtonElement>(null)
  const tray = React.useRef<HTMLDivElement>(null)
  const trayId = React.useId()
  const placement = useTrayPlacement(open, root)

  const close = React.useCallback((returnFocus: boolean) => {
    setOpen(false)
    setPage("root")
    if (returnFocus) trigger.current?.focus()
  }, [])

  // Each page starts with its first row focused, so the keyboard lands where
  // the eye does.
  React.useEffect(() => {
    if (!open) return
    tray.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus()
  }, [open, page])

  React.useEffect(() => {
    if (!open) return
    const away = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) close(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      if (page === "confirm-full") setPage("approval")
      else if (page === "approval") setPage("root")
      else close(true)
    }
    document.addEventListener("pointerdown", away)
    document.addEventListener("keydown", escape)
    return () => {
      document.removeEventListener("pointerdown", away)
      document.removeEventListener("keydown", escape)
    }
  }, [open, page, close])

  const label = onSignOut ? "More options" : "Add attachment"
  return (
    <div ref={root} className="relative flex shrink-0">
      <ChatComposerAction
        ref={trigger}
        className="nessa-composer-control"
        aria-label={label}
        title={label}
        aria-expanded={open}
        aria-controls={open ? trayId : undefined}
        disabled={disabled && !onSignOut && !approval}
        onClick={() => (open ? close(false) : setOpen(true))}
      >
        <Plus
          aria-hidden="true"
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
          className="nessa-composer-tray absolute bottom-[calc(100%+10px)] left-0 z-50 w-[21rem] overflow-hidden rounded-[22px] border border-border/60 bg-popover p-1.5 text-popover-foreground shadow-xl"
        >
          {page === "root" ? (
            <div key="root" className="nessa-composer-tray-page flex flex-col">
              <button
                type="button"
                className={ROW}
                disabled={disabled}
                onClick={() => {
                  close(false)
                  onChoose()
                }}
              >
                <span className="flex-1">Add files</span>
              </button>
              {approval ? (
                <button type="button" className={ROW} onClick={() => setPage("approval")}>
                  <span className="flex-1">Tool approval</span>
                  {/* Full access stays red wherever it is named. */}
                  <span
                    className={
                      approval.mode === "full"
                        ? "text-destructive"
                        : "text-muted-foreground"
                    }
                  >
                    {APPROVAL_MODE_TEXT[approval.mode].name}
                  </span>
                  <ChevronRight
                    aria-hidden="true"
                    className="size-4 text-muted-foreground"
                  />
                </button>
              ) : null}
              {onSignOut ? (
                <button
                  type="button"
                  className={ROW}
                  onClick={() => {
                    close(false)
                    onSignOut()
                  }}
                >
                  <span className="flex-1">Sign out</span>
                </button>
              ) : null}
            </div>
          ) : page === "approval" && approval ? (
            <div key="approval" className="nessa-composer-tray-page flex flex-col">
              <button
                type="button"
                onClick={() => setPage("root")}
                className="flex items-center gap-1 self-start rounded-[12px] px-2 py-2 nessa-text-3 text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:text-foreground"
              >
                <ChevronLeft aria-hidden="true" className="size-4" />
                Tool approval
              </button>
              <div role="radiogroup" aria-label="Tool approval" className="flex flex-col">
                {APPROVAL_MODES.map((mode) => {
                  const offered = approval.modes.includes(mode)
                  const chosen = approval.mode === mode
                  return (
                    <button
                      key={mode}
                      type="button"
                      role="radio"
                      aria-checked={chosen}
                      disabled={!offered}
                      className={`${ROW} items-start`}
                      onClick={() => {
                        // Turning full access on asks first; every other
                        // choice, including leaving full access, does not.
                        if (mode === "full" && !chosen) {
                          setPage("confirm-full")
                          return
                        }
                        setPage("root")
                        if (!chosen) approval.onChange(mode)
                      }}
                    >
                      <span className="flex flex-1 flex-col gap-0.5">
                        <span
                          className={mode === "full" ? "text-destructive" : undefined}
                        >
                          {APPROVAL_MODE_TEXT[mode].name}
                        </span>
                        <span className="nessa-text-2 text-muted-foreground">
                          {offered
                            ? APPROVAL_MODE_TEXT[mode].says
                            : "Not available for this agent."}
                        </span>
                      </span>
                      {chosen ? (
                        <Check
                          aria-hidden="true"
                          className="mt-0.5 size-4 text-foreground"
                        />
                      ) : null}
                    </button>
                  )
                })}
              </div>
            </div>
          ) : page === "confirm-full" && approval ? (
            <div
              key="confirm-full"
              role="alertdialog"
              aria-labelledby={`${trayId}-full-title`}
              className="nessa-composer-tray-page flex flex-col gap-3 p-2"
            >
              <p
                id={`${trayId}-full-title`}
                className="m-0 px-1.5 pt-1 nessa-text-5 font-semibold text-foreground"
              >
                Turn on full access?
              </p>
              {/* Tinted red, so the card says "risk" before a word is read. */}
              <ul className="m-0 flex flex-col gap-2 rounded-[16px] border border-destructive/25 bg-destructive/10 px-4 py-3 nessa-text-3 text-foreground">
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
                  onClick={() => setPage("approval")}
                  className="flex-1 whitespace-nowrap rounded-full bg-muted px-4 py-2.5 nessa-text-4 font-medium text-foreground outline-none transition-colors hover:bg-accent focus-visible:bg-accent"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setPage("root")
                    approval.onChange("full")
                  }}
                  className="flex-1 whitespace-nowrap rounded-full bg-destructive px-4 py-2.5 nessa-text-4 font-medium text-white outline-none transition-opacity hover:opacity-90 focus-visible:opacity-90"
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
