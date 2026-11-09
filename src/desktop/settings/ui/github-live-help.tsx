import { useCallback, useEffect, useId, useRef, useState } from "react"
import { PopoverSurface } from "@nessa-ui/react/popover-surface"
import { IconButton } from "../../ui/icon-button"
import { placeTooltip } from "../../model/tooltip-placement"
import { RowAction } from "./settings-controls"
import "./github-live-help.css"

/** A non-modal, light-dismiss help surface. The browser owns outside-click
 * dismissal; Escape is stopped here so it does not also close Settings. */
export function GithubLiveHelp({
  refresh,
  disabled,
}: {
  refresh?: () => void
  disabled?: boolean
}) {
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
    document.addEventListener("scroll", place, true)
    return () => {
      window.removeEventListener("resize", place)
      document.removeEventListener("scroll", place, true)
    }
  }, [open, place])
  return (
    <>
      <IconButton
        ref={trigger}
        icon={
          <span className="github-live-help-mark" aria-hidden="true">
            ?
          </span>
        }
        label="About live GitHub events"
        size="sm"
        popoverTarget={id}
        aria-expanded={open}
        onClick={(event) => {
          event.preventDefault()
          const target = panel.current
          if (!target) return
          if (target.matches(":popover-open")) {
            target.hidePopover()
            return
          }
          target.showPopover()
          place()
          target.focus({ preventScroll: true })
        }}
      />
      <PopoverSurface
        ref={panel}
        id={id}
        popover="auto"
        role="dialog"
        aria-label="About live GitHub events"
        tabIndex={-1}
        className="github-live-help-panel"
        onToggle={(event) => {
          const shown = event.newState === "open"
          setOpen(shown)
          if (shown) place()
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault()
            event.stopPropagation()
            panel.current?.hidePopover()
            trigger.current?.focus({ preventScroll: true })
          }
        }}
      >
        <strong>How live events work</strong>
        <ul>
          <li>
            Uses your existing Nessa GitHub sign-in and the repositories you select on
            GitHub.
          </li>
          <li>
            Receives issue, pull request and CI updates. Events do not start agent actions
            yet.
          </li>
          <li>
            Payloads pass through Nessa’s Cloudflare relay, with temporary storage for
            offline catch-up and a seven-day retention target, within capacity.
          </li>
          <li>
            Disabling or disconnecting clears this gateway’s local event inbox. Cloud
            retention is shared with your other devices.
          </li>
          <li>To revoke GitHub access, use GitHub Settings → Applications.</li>
        </ul>
        {refresh && (
          <RowAction disabled={disabled} onClick={refresh}>
            Refresh repositories
          </RowAction>
        )}
      </PopoverSurface>
    </>
  )
}
