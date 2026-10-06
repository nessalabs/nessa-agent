/**
 * An approval's command and its answers, drawn once for every card that asks
 * — the pane's (`approval-card.tsx`) and the Agents overview's peek — and
 * fitted to the card's width by container queries (`approval-card.css`).
 * A label wraps inside its button, and options that do not fit the row
 * continue on the next, so a long or numerous review stays inside the card.
 *
 * The buttons are the review's options (`Approval.options`), each in the
 * review's own words. Deny stays at the left; what allows sits at the right.
 * When the review also offers always, the width arranges those two:
 *
 * | the answers' width | arrangement                                              |
 * | ------------------ | -------------------------------------------------------- |
 * | 380px or more      | Deny at the left; Always Allow and Allow Once at the right |
 * | 280–380px          | one row at the right; "Always Allow" says its first word  |
 * | under 280px        | Allow Once across the width, Always Allow in its menu;    |
 * |                    | Deny, quiet, across the width beneath                     |
 *
 * A review that does not offer always has no such button and no menu.
 *
 * The command breaks only between its words — never inside one, so
 * `--simulate` stays whole — each line after the first hanging under the
 * first word, clear of the `$`; a word longer than the card scrolls
 * sideways instead, its cut edge faded, by the trackpad or — the block then
 * taking the keyboard, in both engines — the arrow keys.
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
import type {
  Approval,
  ApprovalAsk,
  ApprovalChoice,
  ApprovalOption,
  ApprovalOrigin,
} from "../../model/transcript"
import { named, naming, shownName, type Said } from "./said"
import "./approval-card.css"

/**
 * Who asks, as the person is told: the agent by its name, or an app by its
 * server, so that a review an app opened is not put in the agent's mouth
 * (`transcript.test.tsx` O1/O2, `overview.test.tsx` O3, on #436). The one
 * place the asker is worded: the card's head (`approvalHead`) and the
 * overview's row both read it; each says what is asked in its own way. An
 * app's server is a name the window did not choose, isolated wherever it is
 * shown (`said.tsx`).
 */
export function approvalAsker(origin: ApprovalOrigin, agent: string): Said {
  switch (origin.kind) {
    case "agent":
      return [agent]
    case "app":
      return ["The ", named(origin.server), " app"]
  }
}

/**
 * The card's head: who asks, and what — the agent to run a command; an app
 * to run the tool it named, or to send a message as the person. The agent
 * asks only to run tools: the client refuses a view that says otherwise
 * (`conversation-validate.ts`, D19 on #390).
 */
export function approvalHead(
  approval: Pick<Approval, "origin" | "ask">,
  agent: string,
): Said {
  const { origin } = approval
  const asker = approvalAsker(origin, agent)
  if (origin.kind === "agent") return [...asker, " wants to run a command"]
  return approval.ask === "message"
    ? [...asker, " wants to send a message as you"]
    : [...asker, " wants to run ", named(origin.tool)]
}

/**
 * What the Agents overview's row says is asked: a command or tool written out
 * whole, as the row has no card to show it in; a message as the card's head
 * says it (`overview.test.tsx` O3 and D19). An app's tool command is the
 * app's own words, its tool's name among them, and is isolated as a name is
 * (`said.tsx`, E2-1 on #390).
 */
export function approvalRequest(
  approval: Pick<Approval, "origin" | "ask" | "command">,
  agent: string,
): Said {
  const asker = approvalAsker(approval.origin, agent)
  if (approval.ask === "message") return approvalHead(approval, agent)
  return approval.origin.kind === "app"
    ? [...asker, " wants to run ", named(approval.command)]
    : [...asker, ` wants to run ${approval.command}`]
}

/**
 * What Deny and Allow Once do, as their tooltips say it, by what is asked: a
 * message is sent, not run (D19 on #390). Read by the overview's row and its
 * peek (`overview.test.tsx` D19).
 */
export const answerTips: Readonly<
  Record<ApprovalAsk, { readonly deny: string; readonly once: string }>
> = {
  tool: { deny: "Don’t run it", once: "Run it once" },
  message: { deny: "Don’t send it", once: "Send it once" },
}

/**
 * Why a review asks, as the gateway titled it. An app's title repeats the
 * tool and the server (`app_reviews.rs`); each is isolated, and the words
 * around them stay (`approval-request.test.tsx`, E2-1). The agent's title
 * is its own words.
 */
export function approvalReason(approval: Pick<Approval, "origin" | "reason">): Said {
  return approval.origin.kind === "app"
    ? naming(approval.reason, [approval.origin.tool, approval.origin.server])
    : [approval.reason]
}

/** The first word of a label, shown when the card is too narrow for the whole. */
function shortOf(label: string): string {
  const space = label.indexOf(" ")
  return space === -1 ? label : label.slice(0, space)
}

/** The command's words, spaces kept between them. */
function commandWords(text: string, key: string) {
  return text.split(/( +)/).map((part, index) =>
    part.trim() === "" ? (
      part
    ) : (
      <span key={`${key}${index}`} className="workspace-approval-word">
        {part}
      </span>
    ),
  )
}

/**
 * The command, broken only between its words. An app's tool name, when
 * `name` is that tool and the command begins with it, is isolated as a name
 * is; the words after it — a message, as it will be sent — are left as they
 * are (`approval-request.test.tsx`, E2-1).
 */
export function ApprovalCommand({
  command,
  name,
}: {
  command: string
  /** The app's tool, when this command is that app's. */
  name?: string
}) {
  const lead =
    name && name.length > 0 && (command === name || command.startsWith(`${name} `))
      ? name
      : undefined
  const rest = lead === undefined ? command : command.slice(lead.length)
  const block = useRef<HTMLPreElement>(null)
  // Where a word is wider than the card, the block scrolls and fades the edge it cuts.
  useLayoutEffect(() => {
    const element = block.current
    if (!element || typeof ResizeObserver === "undefined") return
    const mark = () => {
      const over = element.scrollWidth - element.clientWidth > 1
      element.toggleAttribute("data-overflow", over)
      // Scrolled by the keys too, where it scrolls at all: WebKit does not
      // make a scrolling block focusable on its own.
      if (over) element.tabIndex = 0
      else element.removeAttribute("tabindex")
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
    <pre
      ref={block}
      className="workspace-approval-command"
      aria-label="Command"
      onKeyDown={(event) => {
        // The arrows scroll it, as they would any scrolling block, in both engines.
        const steps: Readonly<Record<string, number>> = { ArrowLeft: -1, ArrowRight: 1 }
        const step = Object.hasOwn(steps, event.key) ? steps[event.key] : 0
        if (!step || event.metaKey || event.ctrlKey || event.altKey) return
        const element = event.currentTarget
        if (element.scrollWidth - element.clientWidth <= 1) return
        event.preventDefault()
        element.scrollBy({ left: step * 48 })
      }}
    >
      <span className="workspace-approval-prompt" aria-hidden="true">
        ${" "}
      </span>
      {lead === undefined ? null : <bdi>{commandWords(shownName(lead), "name-")}</bdi>}
      {commandWords(rest, "word-")}
    </pre>
  )
}

/** The answers the review offers, arranged for the width they have. */
export function ApprovalActions({
  options,
  disabled,
  onAnswer,
  tips = {},
}: {
  /** The review's answers. Each is a button; an answer it does not list is not drawn. */
  options: readonly ApprovalOption[]
  disabled: boolean
  /** `at`: when the answer was made (the event's `timeStamp`, on `performance.now()`'s clock). */
  onAnswer: (option: ApprovalOption, at: number) => void
  /** Each answer's tooltip, where the card offers one (its shortcut, say). */
  tips?: Partial<Record<ApprovalChoice, TooltipAttributes>>
}) {
  const denies = options.filter((option) => option.choice === "deny")
  const always = options.filter((option) => option.choice === "always")
  const once = options.filter((option) => option.choice === "once")
  const answer = (option: ApprovalOption) => (event: { timeStamp: number }) =>
    onAnswer(option, event.timeStamp)
  return (
    <div
      className="workspace-approval-answers"
      onKeyDown={(event) => {
        // One press, one act: a held key's repeats press nothing — not these
        // buttons, nor what the keyboard lands on once they go (`takesAnswerKey`
        // in `model/overview/walk.ts` owns the rule).
        if (event.repeat) event.preventDefault()
      }}
    >
      <div className="workspace-approval-actions">
        {denies.map((option) => (
          <button
            key={option.id}
            type="button"
            className="workspace-button workspace-approval-deny"
            data-answer={option.choice}
            disabled={disabled}
            {...tips.deny}
            onClick={answer(option)}
          >
            {option.label}
          </button>
        ))}
        {always.length > 0 || once.length > 0 ? (
          <div className="workspace-approval-allow">
            {always.map((option) => (
              <button
                key={option.id}
                type="button"
                className="workspace-button workspace-approval-always"
                aria-label={option.label}
                data-answer={option.choice}
                disabled={disabled}
                {...tips.always}
                onClick={answer(option)}
              >
                <span className="workspace-approval-long" aria-hidden="true">
                  {option.label}
                </span>
                <span className="workspace-approval-short" aria-hidden="true">
                  {shortOf(option.label)}
                </span>
              </button>
            ))}
            {once.map((option) => (
              <button
                key={option.id}
                type="button"
                className="workspace-button workspace-approval-once"
                data-answer={option.choice}
                data-primary
                disabled={disabled}
                {...tips.once}
                onClick={answer(option)}
              >
                {option.label}
              </button>
            ))}
            {always.length > 0 ? (
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
                  {always.map((option) => (
                    <MenuItem key={option.id} onSelect={answer(option)}>
                      {option.label}
                    </MenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  )
}
