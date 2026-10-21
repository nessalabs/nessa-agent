import { deadlinePendingLine } from "@nessa/client"
import { memo, useEffect, useId, useState } from "react"
import { useNow } from "../../../../clock"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { approve, deny, retryReview } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { useRememberedValue } from "../../adapters/store/remembered-session"
import { selectAnswer } from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { agentName, agentOf, type ModelRef } from "../../model/workspace-index"
import { shownWhole, type Approval, type ApprovalDeadline } from "../../model/transcript"
import { useClipboardWrite } from "../../../../conversation"
import { failureCopy } from "../failure-copy"
import { commandGist, type CommandGist } from "./approval-gist"
import {
  ApprovalActions,
  ApprovalCommand,
  approvalHead,
  approvalReason,
} from "./approval-request"
import { Saying, shownJson, shownName } from "./said"

/** The last stretch of a deadline, when its ring says what it will do. */
const endingSeconds = 10

/** How many sites and programs the gist names before it counts the rest. */
const shownHosts = 2
const shownPrograms = 4

/** A script longer than `foldAfter` lines shows its first `foldedLines` until Show All. */
const foldAfter = 6
const foldedLines = 4

/**
 * What waits on the person: a command the agent — or an MCP App, which the
 * head then names (`approvalHead`) — waits to run, and its answers,
 * arranged for the card's width (`approval-request.tsx`). Every command is
 * shown as it is: an agent's, one step per line where its quotes allow,
 * under a row naming the sites and programs it names (`approval-gist.ts`),
 * which points at the command and never stands in for it; an app's tool and
 * a message to send as the person, whole. While an answer is on its way the
 * buttons rest; a failed answer says why.
 */
export const ApprovalCard = memo(function ApprovalCard({
  sessionId,
  approval,
  model,
}: {
  sessionId: string
  approval: Approval
  model: ModelRef
}) {
  const dispatch = useWorkspaceDispatch()
  // The source drops the answer with the session. The card stays until the box goes.
  const answerLive = useWorkspaceSelector((state) =>
    selectAnswer(state, sessionId, approval.id),
  )
  const answer = useRememberedValue(sessionId, answerLive)
  const waiting = answering(answer, approval.id)
  const gist =
    approval.origin.kind === "agent" && approval.ask === "tool" && shownWhole(approval)
      ? commandGist(approval.command)
      : undefined
  return (
    <div
      className="workspace-approval"
      role="group"
      aria-label="Approval needed"
      data-origin={approval.origin.kind}
      data-ask={approval.ask}
    >
      <div className="workspace-approval-top">
        <div className="workspace-approval-head">
          <span className="workspace-approval-mark" aria-hidden="true">
            <DesktopIcon name="needsYou" />
          </span>
          <span className="workspace-approval-head-words">
            <Saying said={approvalHead(approval, agentName(agentOf(model)))} />
          </span>
        </div>
        {approval.deadline ? <DeadlineRing deadline={approval.deadline} /> : null}
      </div>
      {repeatsCommand(approval, gist) ? null : (
        <p className="workspace-approval-reason">
          <Saying said={approvalReason(approval)} />
        </p>
      )}
      {/* While the review is read, `command` is only the tool's name: nothing to show yet. */}
      {approval.loading === "reading" ? (
        <p className="workspace-approval-loading" role="status">
          Loading this review before you can answer…
        </p>
      ) : approval.loading === "failed" ? (
        <p className="workspace-approval-loading" role="status">
          This review could not be loaded. Trying again…{" "}
          <button
            type="button"
            className="workspace-button workspace-approval-retry"
            onClick={() =>
              void dispatch(retryReview({ sessionId, approvalId: approval.id }))
            }
          >
            Try again
          </button>
        </p>
      ) : gist ? (
        <ApprovalScript command={approval.command} gist={gist} />
      ) : (
        <ApprovalCommand
          command={approval.command}
          name={approval.origin.kind === "app" ? approval.origin.tool : undefined}
        />
      )}
      {answer?.failure ? (
        <p className="workspace-approval-failure" role="status">
          {failureCopy(answer.failure)}
        </p>
      ) : null}
      <ApprovalActions
        options={approval.options}
        // Not before all of what it allows is shown (`Approval.loading`).
        disabled={waiting || !shownWhole(approval)}
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
    </div>
  )
})

/**
 * Whether the agent's title for its review only names the tool it calls
 * (`mcp__nessa__shell`, or nothing at all), which the card already shows:
 * then there is no reason line. A title in words of its own ("Runs the
 * tests.") is the summary the person reads first.
 */
function repeatsCommand(approval: Approval, gist: CommandGist | undefined): boolean {
  if (!gist) return false
  const reason = approval.reason.trim()
  const lead = gist.tool ?? approval.command.trim().split(/\s/, 1)[0]
  return reason === "" || reason === lead
}

/**
 * The deadline, quietly: the seconds left beside a ring that empties as they
 * run out. In the last ten seconds it says what will happen ("Denies in
 * 6s"). The whole sentence (`deadlinePendingLine`) is its accessible name
 * and its tooltip.
 */
function DeadlineRing({ deadline }: { deadline: ApprovalDeadline }) {
  const now = useNow(1000)
  const left = Math.max(0, Math.ceil((deadline.dueAtMs - now) / 1000))
  const whole = Math.max(1, Math.ceil(deadline.waitMs / 1000))
  const ending = left <= endingSeconds
  const verb = deadline.effect === "allow" ? "Allows" : "Denies"
  const words = !ending
    ? `${left}s`
    : left > 0
      ? `${verb} in ${left}s`
      : deadline.effect === "allow"
        ? "Allowing"
        : "Denying"
  const sentence = deadlinePendingLine(deadline, now)
  const rim = 2 * Math.PI * 5.5
  return (
    <p
      className="workspace-approval-deadline"
      data-deadline={deadline.effect}
      data-ending={ending ? "" : undefined}
      role="timer"
      aria-label={sentence}
      {...tooltip(sentence)}
    >
      <span aria-hidden="true">{words}</span>
      <svg viewBox="0 0 14 14" aria-hidden="true">
        <circle className="workspace-approval-deadline-track" cx="7" cy="7" r="5.5" />
        <circle
          className="workspace-approval-deadline-left"
          cx="7"
          cy="7"
          r="5.5"
          strokeDasharray={rim}
          strokeDashoffset={rim * (1 - Math.min(1, left / whole))}
        />
      </svg>
    </p>
  )
}

/**
 * An agent's command: a row naming the sites it names and the programs it
 * names; then the script itself,
 * the tool it calls and that tool's other input, and a way to copy the
 * command exactly as it came. A long script shows its first lines and says
 * how many there are until Show All; nothing else is ever folded away.
 */
function ApprovalScript({ command, gist }: { command: string; gist: CommandGist }) {
  const [unfolded, setUnfolded] = useState(false)
  const script = useId()
  const hosts = gist.hosts.slice(0, shownHosts)
  const programs = gist.programs.slice(0, shownPrograms)
  const moreHosts = gist.hosts.length - hosts.length
  const morePrograms = gist.programs.length - programs.length
  const shown = gist.script ?? command
  // A newline that only ends the script starts no line of its own.
  const lines = shown.replace(/\n$/, "").split("\n")
  const folds = lines.length > foldAfter
  const folded = folds && !unfolded
  // One program run once, and no site, is the command's first word: the row would only repeat it.
  const names = hosts.length > 0 || programs.length > 1 || (programs[0]?.count ?? 0) > 1
  return (
    <>
      {names ? (
        <div className="workspace-approval-gist">
          {hosts.length > 0 ? (
            <span className="workspace-approval-fact" aria-label="Sites named">
              {hosts.map((host) => (
                <span key={host} className="workspace-approval-site">
                  {/* No site icon: asking the site for it would reach a host the
                    command names before the person has allowed anything. */}
                  <DesktopIcon name="site" />
                  <bdi>{shownName(host)}</bdi>
                </span>
              ))}
              {moreHosts > 0 ? (
                <span className="workspace-approval-more-count">+{moreHosts}</span>
              ) : null}
            </span>
          ) : null}
          {programs.length > 0 ? (
            <span className="workspace-approval-fact" aria-label="Programs named">
              <DesktopIcon name="terminal" />
              {programs.map((program) => (
                <span key={program.name} className="workspace-approval-program">
                  <bdi>{shownName(program.name)}</bdi>
                  {program.count > 1 ? <i>×{program.count}</i> : null}
                </span>
              ))}
              {morePrograms > 0 ? (
                <span className="workspace-approval-more-count">+{morePrograms}</span>
              ) : null}
            </span>
          ) : null}
        </div>
      ) : null}
      <div
        id={script}
        className="workspace-approval-script"
        data-folded={folded ? "" : undefined}
      >
        <ApprovalCommand
          command={folded ? lines.slice(0, foldedLines).join("\n") : shown}
        />
      </div>
      <div className="workspace-approval-meta-row">
        <dl className="workspace-approval-meta">
          {gist.tool ? (
            <div>
              <dt>Tool</dt>
              <dd>
                <bdi>{shownName(gist.tool)}</bdi>
              </dd>
            </div>
          ) : null}
          {gist.script !== undefined
            ? gist.fields.map(([name, value]) => (
                <div key={name}>
                  <dt>
                    <bdi>{shownName(name)}</bdi>
                  </dt>
                  <dd>{shownJson(value)}</dd>
                </div>
              ))
            : null}
        </dl>
        {folds ? (
          <button
            type="button"
            className="workspace-approval-disclose"
            aria-expanded={unfolded}
            aria-controls={script}
            onClick={() => setUnfolded((open) => !open)}
          >
            {unfolded ? "Show Less" : `Show All ${lines.length} Lines`}
          </button>
        ) : null}
        <CopyCommand command={command} />
      </div>
    </>
  )
}

/** Copies the command exactly as the agent sent it, through the host's clipboard, and says that it did. */
function CopyCommand({ command }: { command: string }) {
  const write = useClipboardWrite()
  const [said, setSaid] = useState<"copied" | "refused">()
  useEffect(() => {
    if (!said) return
    const timer = setTimeout(() => setSaid(undefined), 1400)
    return () => clearTimeout(timer)
  }, [said])
  if (!write) return null
  return (
    <button
      type="button"
      className="workspace-approval-copy"
      onClick={() =>
        void write(command).then(
          () => setSaid("copied"),
          () => setSaid("refused"),
        )
      }
    >
      {said === "copied"
        ? "Copied"
        : said === "refused"
          ? "Couldn’t Copy"
          : "Copy Command"}
    </button>
  )
}
