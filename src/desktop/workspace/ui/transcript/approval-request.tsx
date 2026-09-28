/**
 * An approval's command and its answers, drawn once for every card that asks
 * — the pane's (`approval-card.tsx`) and the Agents overview's peek — and
 * fitted to the card's width by container queries (`approval-card.css`), so
 * no button is ever left alone on a row:
 *
 * | the answers' width | arrangement                                              |
 * | ------------------ | -------------------------------------------------------- |
 * | 380px or more      | Deny at the left; Always Allow and Allow Once at the right |
 * | 280–380px          | one row at the right; "Always Allow" says "Always"        |
 * | under 280px        | Allow Once across the width, Always Allow in its menu;    |
 * |                    | Deny, quiet, across the width beneath                     |
 *
 * The command breaks only between its words — never inside one, so
 * `--simulate` stays whole — each line after the first hanging under the
 * first word, clear of the `$`; a word longer than the card scrolls
 * sideways instead, its cut edge faded.
 */
import { useLayoutEffect, useRef } from "react"
import { DesktopIcon } from "../../../ui/icons"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuItem,
} from "../../../ui/menu"
import type { TooltipAttributes } from "../../../ui/tooltip"
import "./approval-card.css"

/** An answer to an approval, as its buttons give it. */
export type ApprovalChoice = "deny" | "always" | "once"

/** The command, broken only between its words. */
export function ApprovalCommand({ command }: { command: string }) {
  const block = useRef<HTMLPreElement>(null)
  // Where a word is wider than the card, the block scrolls and fades the edge it cuts.
  useLayoutEffect(() => {
    const element = block.current
    if (!element || typeof ResizeObserver === "undefined") return
    const mark = () => {
      const over = element.scrollWidth - element.clientWidth > 1
      element.toggleAttribute("data-overflow", over)
      element.toggleAttribute("data-scrolled", over && element.scrollLeft > 1)
      element.toggleAttribute(
        "data-at-end",
        !over || element.scrollLeft + element.clientWidth >= element.scrollWidth - 1,
      )
    }
    const observer = new ResizeObserver(mark)
    observer.observe(element)
    element.addEventListener("scroll", mark, { passive: true })
    return () => {
      observer.disconnect()
      element.removeEventListener("scroll", mark)
    }
  }, [])
  return (
    <pre ref={block} className="workspace-approval-command">
      <span className="workspace-approval-prompt" aria-hidden="true">
        ${" "}
      </span>
      {command.split(/( +)/).map((part, index) =>
        part.trim() === "" ? (
          part
        ) : (
          <span key={index} className="workspace-approval-word">
            {part}
          </span>
        ),
      )}
    </pre>
  )
}

/** The answers, arranged for the width they have. */
export function ApprovalActions({
  disabled,
  onAnswer,
  tips = {},
}: {
  disabled: boolean
  onAnswer: (choice: ApprovalChoice) => void
  /** Each answer's tooltip, where the card offers one (its shortcut, say). */
  tips?: Partial<Record<ApprovalChoice, TooltipAttributes>>
}) {
  return (
    <div className="workspace-approval-answers">
      <div className="workspace-approval-actions">
        <button
          type="button"
          className="workspace-button workspace-approval-deny"
          disabled={disabled}
          {...tips.deny}
          onClick={() => onAnswer("deny")}
        >
          Deny
        </button>
        <div className="workspace-approval-allow">
          <button
            type="button"
            className="workspace-button workspace-approval-always"
            aria-label="Always Allow"
            disabled={disabled}
            {...tips.always}
            onClick={() => onAnswer("always")}
          >
            <span className="workspace-approval-long" aria-hidden="true">
              Always Allow
            </span>
            <span className="workspace-approval-short" aria-hidden="true">
              Always
            </span>
          </button>
          <button
            type="button"
            className="workspace-button workspace-approval-once"
            data-primary
            disabled={disabled}
            {...tips.once}
            onClick={() => onAnswer("once")}
          >
            Allow Once
          </button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="workspace-button workspace-approval-more"
                data-primary
                aria-label="More Ways to Allow"
                disabled={disabled}
              >
                <DesktopIcon name="chevronDown" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <MenuItem onSelect={() => onAnswer("always")}>Always Allow</MenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
    </div>
  )
}
