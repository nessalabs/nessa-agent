/**
 * A shell fence in a reply, with a Run button under it. Run starts the
 * command in a new terminal in the conversation's folder, on the agent's
 * machine, and its output shows here as one real grid: a terminal icon
 * opens that terminal in a pane, a stop icon sends Control-C. When it ends
 * the bar says `exit N · duration` and offers Run again. Only the person
 * runs a block: agents never press Run and never read a terminal
 * (`docs/design/terminals.md`).
 */
import { memo, useCallback, useState } from "react"
import { Button } from "@nessa-ui/react/button"
import { CodeBlock } from "@nessa-ui/react/code-block"
import type { ReplyFence } from "@nessa-ui/react/reply-markdown"
import { TerminalOpenError, type TerminalOpenFailure } from "../../application/ports"
import {
  interruptTerminal,
  openTerminal,
  runInConversation,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { latestRun, runCommand, runEndWords, runRequestId } from "../../model/run-block"
import { DesktopIcon } from "../../../ui/icons"
import { IconButton } from "../../../ui/icon-button"
import { TerminalView } from "../terminals/terminal-view"
import "./run-block.css"

/** The grid a Run opens at, before its view measures the box it has. */
const runFit = { cols: 100, rows: 10 }

const failureWords: Record<TerminalOpenFailure | "too-long" | "empty", string> = {
  unavailable: "Terminals aren't available on this computer.",
  limit: "Too many terminals are open. Close one and try again.",
  elsewhere: "This chat runs elsewhere, so it can't run here yet.",
  "folder-missing": "The chat's folder is gone.",
  "not-found": "This chat is gone.",
  "start-failed": "The shell didn't start.",
  "too-long": "This is too long to run. Copy it into a terminal instead.",
  empty: "There's nothing to run here.",
}

export const RunBlock = memo(function RunBlock({
  fence,
  conversationId,
  base,
}: {
  fence: ReplyFence
  conversationId: string
  /** The request id its runs share (`runRequestBase`). */
  base: string
}) {
  const dispatch = useWorkspaceDispatch()
  const run = useWorkspaceSelector((state) =>
    latestRun(state.workspace.terminals, conversationId, base),
  )
  const [starting, setStarting] = useState(false)
  const [failure, setFailure] = useState<keyof typeof failureWords | null>(null)
  const start = useCallback(() => {
    const planned = runCommand(fence.code, fence.language)
    if ("refused" in planned) {
      setFailure(planned.refused)
      return
    }
    setFailure(null)
    setStarting(true)
    dispatch(
      runInConversation({
        conversationId,
        command: planned.command,
        requestId: runRequestId(base, crypto.randomUUID()),
        fit: runFit,
      }),
    )
      .catch((error: unknown) => {
        setFailure(error instanceof TerminalOpenError ? error.reason : "unavailable")
      })
      .finally(() => setStarting(false))
  }, [dispatch, conversationId, base, fence.code, fence.language])
  const running = run?.state === "running"
  const ended = run ? runEndWords(run) : undefined
  return (
    <div
      className="workspace-run-block"
      data-run-block
      data-run-state={run ? run.state : "idle"}
    >
      <CodeBlock code={fence.code} language={fence.language} />
      <div className="workspace-run-bar">
        {running ? (
          <span className="workspace-run-status">Running</span>
        ) : (
          <Button
            variant="tinted"
            shape="pill"
            size="24"
            disabled={starting}
            onClick={start}
            data-run-button
          >
            <DesktopIcon name="resume" size={12} aria-hidden />
            {run ? "Run again" : "Run"}
          </Button>
        )}
        {ended ? (
          <span className="workspace-run-status" data-run-exit={run?.exitStatus ?? ""}>
            {ended}
          </span>
        ) : null}
        {failure ? (
          <span className="workspace-run-failure" role="status">
            {failureWords[failure]}
          </span>
        ) : null}
        <span className="workspace-run-spacer" />
        {run ? (
          <IconButton
            icon="terminal"
            label="Open in Pane"
            onClick={() => dispatch(openTerminal({ terminalId: run.id }))}
          />
        ) : null}
        {running ? (
          <IconButton
            icon="stop"
            label="Stop"
            onClick={() => dispatch(interruptTerminal(run.id))}
          />
        ) : null}
      </div>
      {run ? (
        <div className="workspace-run-output">
          <TerminalView terminalId={run.id} label={`Output of ${run.title}`} />
        </div>
      ) : null}
    </div>
  )
})
