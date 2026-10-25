/**
 * What a waiting row in the session list asks, answered from the list: the
 * transcript card's own parts, compact (`transcript/approval-request.tsx`).
 * Under the row's agent and title, who asks and the command, then its
 * answers: the refusal and the primary allow, every other in the menu
 * beside them. A short request in printable ASCII (`allowableFromList`)
 * is drawn in full and may be allowed; any other is a preview of three
 * lines with a fade, offering its refusals and Review in Chat. A question shows the
 * question card. The first request is shown; more wait as "+N more", in the
 * conversation. While answered, the card leaves on the motion's curve; a
 * refused answer says why in place.
 *
 * The list reads and keeps the conversations it asks about
 * (`listAskIds`, `workspace-state.ts`), so what is shown here is what an
 * answer answers (`onScreen`).
 */
import { memo } from "react"
import { approve, deny, openSession } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectAnswer,
  selectSession,
  selectTranscript,
} from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { shownWhole, type Approval } from "../../model/transcript"
import { agentName, agentOf } from "../../model/workspace-index"
import { failureCopy } from "../failure-copy"
import {
  ApprovalActions,
  ApprovalCommand,
  approvalHead,
  drawnCommand,
} from "../transcript/approval-request"
import { approvalGist } from "../transcript/approval-card"
import { QuestionCard } from "../transcript/question-card"
import { Saying, shownName } from "../transcript/said"
import "../transcript/transcript.css"
import "./list-request.css"
import type { CommandGist } from "../transcript/approval-gist"

/** The most lines of a command the list draws whole. */
export const listShownLines = 3
/**
 * The monospace columns a card's command holds at the list's narrowest
 * width (`columns.mjs` measures it, `list-request-narrow`).
 */
export const listShownColumns = 21
/** The longest tool name the list allows under. */
export const listShownToolChars = 48

/** Printable ASCII and line breaks: text whose every character draws one column wide. */
const printable = /^[\x20-\x7e\n]*$/
/** A name in printable ASCII, on one line, at most `listShownToolChars` long. */
const plainName = (name: string): boolean =>
  /^[\x20-\x7e]*$/.test(name) && name.length <= listShownToolChars

/**
 * How many lines `text` takes at `columns`, broken as the card breaks it:
 * between words, spaces hanging at a line's end, and a word longer than a
 * line anywhere in it. The prompt (`$ `) hangs before the first line, and a
 * first word too long for that line moves below it, as after any word.
 */
export function wrappedLines(text: string, columns: number): number {
  let lines = 0
  let prompt = true
  for (const line of text.split("\n")) {
    lines += 1
    let at = 0
    for (const part of line.split(/( +)/)) {
      if (part === "") continue
      if (part.startsWith(" ")) {
        at += part.length
        continue
      }
      const after = at > 0 || prompt
      prompt = false
      if (after && at + part.length > columns) {
        lines += 1
        at = 0
      }
      let rest = part.length
      while (at + rest > columns) {
        lines += 1
        rest -= columns - at
        at = 0
      }
      at += rest
    }
  }
  return lines
}

/**
 * Whether the list may allow a request: only one it can only draw whole,
 * decided from the request alone, never from how it happened to lay out.
 * Its command as the card reads it (the script `approvalGist` reads, or
 * the command) and every name the card shows (the agent's tool, or the
 * app and its tool) are printable ASCII, so no character is wider than a
 * column, joins another, reorders the line, or is drawn as an escape that
 * reads like typed text; the command as drawn (`drawnCommand`) takes at
 * most `listShownLines` lines at the list's narrowest; each name is at most
 * `listShownToolChars` long; and the tool's input holds nothing but the
 * command. Any other
 * request is refused from the list, or read whole in the chat.
 */
export function allowableFromList(
  approval: Approval,
  gist: CommandGist | undefined,
): boolean {
  if (!shownWhole(approval)) return false
  if (gist && gist.fields.length > 0) return false
  const names =
    approval.origin.kind === "app"
      ? [approval.origin.server, approval.origin.tool]
      : [gist?.tool ?? ""]
  const read = gist?.script ?? approval.command
  const drawn = drawnCommand(
    read,
    approval.origin.kind === "app" ? approval.origin.tool : undefined,
  ).replace(/\n$/, "")
  return (
    printable.test(read) &&
    names.every(plainName) &&
    wrappedLines(drawn, listShownColumns) <= listShownLines
  )
}

export const ListRequest = memo(function ListRequest({
  sessionId,
}: {
  sessionId: string
}) {
  const dispatch = useWorkspaceDispatch()
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const transcript = useWorkspaceSelector((state) => selectTranscript(state, sessionId))
  const approvals = transcript?.approvals ?? []
  const questions = transcript?.questions ?? []
  const approval = approvals[0] ?? null
  const answer = useWorkspaceSelector((state) =>
    approval ? selectAnswer(state, sessionId, approval.id) : undefined,
  )
  if (!session) return null
  const asks = approvals.length + questions.length
  const more =
    asks > 1 ? <span className="workspace-request-more">+{asks - 1} more</span> : null
  if (!approval) {
    const question = questions[0]
    if (!question) return null
    return (
      <div
        className="workspace-request"
        role="group"
        aria-label={`Question from ${session.title}`}
        data-request="question"
      >
        <QuestionCard sessionId={sessionId} question={question} />
        {more}
      </div>
    )
  }
  const sent = answering(answer, approval.id)
  // The transcript card's own reading of the command (`approvalGist`).
  const gist = approvalGist(approval)
  const answerable = shownWhole(approval) && !sent
  // Shown whole, it may be allowed here; else refused, or read in the chat.
  const whole = allowableFromList(approval, gist)
  return (
    <div
      className="workspace-request"
      role="group"
      aria-label={`Request from ${session.title}`}
      data-request="approval"
      data-answering={sent || undefined}
    >
      <p className="workspace-request-head">
        <Saying said={approvalHead(approval, agentName(agentOf(session.model)))} />
      </p>
      <div className="workspace-request-command" data-whole={whole || undefined}>
        {approval.loading ? (
          <p className="workspace-request-loading" role="status">
            {approval.loading === "reading"
              ? "Loading this review…"
              : "This review could not be loaded."}
          </p>
        ) : (
          <ApprovalCommand
            command={gist?.script ?? approval.command}
            name={approval.origin.kind === "app" ? approval.origin.tool : undefined}
          />
        )}
      </div>
      {gist?.tool ? (
        <p className="workspace-request-tool">
          Tool <bdi>{shownName(gist.tool)}</bdi>
        </p>
      ) : null}
      {answer?.failure ? (
        <p className="workspace-request-failure" role="status">
          {failureCopy(answer.failure)}
        </p>
      ) : null}
      <div className="workspace-request-foot">
        {more}
        <ApprovalActions
          compact
          // Not shown whole, it is refused or read whole in the chat, never allowed unseen.
          options={
            whole
              ? approval.options
              : approval.options.filter((option) => option.choice === "deny")
          }
          disabled={!answerable}
          onAnswer={(option) =>
            void dispatch(
              option.choice === "deny"
                ? deny({
                    sessionId,
                    approvalId: approval.id,
                    initiator: "person",
                    optionId: option.id,
                  })
                : approve({
                    sessionId,
                    approvalId: approval.id,
                    scope: option.choice,
                    initiator: "person",
                    optionId: option.id,
                  }),
            )
          }
        />
        {whole ? null : (
          <button
            type="button"
            className="workspace-button workspace-request-review"
            data-primary
            onClick={() => dispatch(openSession({ sessionId }))}
          >
            Review in Chat
          </button>
        )}
      </div>
    </div>
  )
})
