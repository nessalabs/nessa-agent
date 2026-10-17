import { deadlinePendingLine } from "@nessa/client"
import { Favicon } from "@nessa-ui/react/favicon"
import { memo, useEffect, useId, useState } from "react"
import { useNow } from "../../../../clock"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { approve, deny } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectAnswer } from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { agentName, agentOf, type ModelRef } from "../../model/workspace-index"
import type { Approval, ApprovalDeadline } from "../../model/transcript"
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

/**
 * What waits on the person: a command the agent — or an MCP App, which the
 * head then names (`approvalHead`) — waits to run, and its answers,
 * arranged for the card's width (`approval-request.tsx`). An agent's command
 * is said at a glance — the sites it reaches and the programs it runs
 * (`approval-gist.ts`) — and shown whole, one step per line, under Details;
 * an app's tool and a message to send as the person are shown whole at once,
 * as their words are what is asked. While an answer is on its way the
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
  const answer = useWorkspaceSelector((state) =>
    selectAnswer(state, sessionId, approval.id),
  )
  const waiting = answering(answer, approval.id)
  const gist =
    approval.origin.kind === "agent" && approval.ask === "tool"
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
      {gist ? (
        <ApprovalDetails command={approval.command} gist={gist} />
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
        disabled={waiting}
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
 * An agent's command at a glance, and whole under Details — opened at once
 * when the gist cannot read every program the command runs (`plain`): the
 * sites it names, each by its own icon where the site has one, and the programs it
 * runs; opened, its script one step per line, the tool it calls and that
 * tool's other input, and a way to copy the command exactly as it came.
 */
function ApprovalDetails({ command, gist }: { command: string; gist: CommandGist }) {
  // A command the gist cannot read whole opens whole: the summary never stands for it.
  const [open, setOpen] = useState(!gist.plain)
  const details = useId()
  const hosts = gist.hosts.slice(0, shownHosts)
  const programs = gist.programs.slice(0, shownPrograms)
  const moreHosts = gist.hosts.length - hosts.length
  const morePrograms = gist.programs.length - programs.length
  return (
    <>
      <div className="workspace-approval-gist" data-open={open ? "" : undefined}>
        <div className="workspace-approval-facts">
          {hosts.length > 0 ? (
            <span className="workspace-approval-fact" aria-label="Sites named">
              {hosts.map((host) => (
                <span key={host} className="workspace-approval-site">
                  <Favicon
                    href={`https://${host}/`}
                    fallback={<DesktopIcon name="site" />}
                    className="workspace-approval-favicon"
                  />
                  <bdi>{shownName(host)}</bdi>
                </span>
              ))}
              {moreHosts > 0 ? (
                <span className="workspace-approval-more-count">+{moreHosts}</span>
              ) : null}
            </span>
          ) : null}
          {programs.length > 0 ? (
            <span className="workspace-approval-fact" aria-label="Programs">
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
          ) : gist.tool ? (
            <span className="workspace-approval-fact" aria-label="Tool">
              <DesktopIcon name="terminal" />
              <span className="workspace-approval-program">
                <bdi>{shownName(gist.tool)}</bdi>
              </span>
            </span>
          ) : null}
        </div>
        <button
          type="button"
          className="workspace-approval-disclose"
          aria-expanded={open}
          aria-controls={details}
          onClick={() => setOpen((shown) => !shown)}
        >
          Details
          <DesktopIcon name="chevronDown" />
        </button>
      </div>
      <div
        id={details}
        className="workspace-approval-details"
        data-open={open ? "" : undefined}
        inert={!open}
      >
        <div className="workspace-approval-details-body">
          <ApprovalCommand command={gist.script ?? command} />
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
            <CopyCommand command={command} />
          </div>
        </div>
      </div>
    </>
  )
}

/** Copies the command exactly as the agent sent it, and says that it did. */
function CopyCommand({ command }: { command: string }) {
  const [said, setSaid] = useState<"copied" | "refused">()
  useEffect(() => {
    if (!said) return
    const timer = setTimeout(() => setSaid(undefined), 1400)
    return () => clearTimeout(timer)
  }, [said])
  return (
    <button
      type="button"
      className="workspace-approval-copy"
      onClick={() =>
        void navigator.clipboard.writeText(command).then(
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
