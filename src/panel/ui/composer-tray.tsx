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

type Page = "root" | "approval"

const ROW =
  "flex w-full items-center gap-3 rounded-[14px] px-3.5 py-3 text-start nessa-text-4 text-foreground outline-none transition-colors hover:bg-accent focus-visible:bg-accent disabled:pointer-events-none disabled:opacity-50"

/**
 * The composer's +: a small tray that rises above the composer.
 *
 * Each entry is one row. A row with choices of its own — tool approval — takes
 * the tray over in place, with a way back, rather than opening a menu beside
 * it: the panel is narrow, and a side menu would open off its edge.
 *
 * It sits inside the pill composer, whose box is its positioning parent, so the
 * tray's bottom edge follows the composer's top as the draft grows.
 */
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
      if (page === "approval") setPage("root")
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
    <div ref={root} className="contents">
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
          className="nessa-composer-tray absolute bottom-[calc(100%+10px)] left-0 z-50 w-[21rem] max-w-full overflow-hidden rounded-[22px] border border-border/60 bg-popover p-1.5 text-popover-foreground shadow-xl"
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
                  <span className="text-muted-foreground">
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
          ) : approval ? (
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
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
