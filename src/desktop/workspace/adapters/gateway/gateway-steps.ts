import {
  clientDueAtMs,
  type ConversationTool,
  type ConversationToolChange,
} from "@nessa/client"
import type { StepDiff, StepKind, StepPart, StepStatus } from "../../model/transcript"

/** What a tool call did, in the window's four kinds; the provider's other kinds read as run. */
export function stepKind(kind: ConversationTool["kind"]): StepKind {
  switch (kind) {
    case "read":
      return "read"
    case "edit":
    case "delete":
    case "move":
      return "edit"
    case "search":
    case "fetch":
      return "search"
    default:
      return "run"
  }
}

/**
 * Where a call is. The gateway says pending, running, completed or failed; a
 * call still pending or running once its turn ended never settled, so it is
 * `stopped`. Any status this window does not know reads as finished.
 */
function stepStatus(status: string, turnEnded: boolean): StepStatus {
  switch (status) {
    case "pending":
    case "running":
      return turnEnded ? "stopped" : status
    case "failed":
      return "failed"
    default:
      return "completed"
  }
}

/**
 * The file a call changed, as its trace draws it: the first the gateway
 * listed, a file the agent created read as empty before. The trace shows one
 * file's diff; the counts below are the call's across them all.
 */
export function stepDiff(
  changes: readonly ConversationToolChange[],
): StepDiff | undefined {
  const [first] = changes
  return (
    first && {
      filename: first.path,
      from: first.before ?? "",
      to: first.after,
      ...(first.cut ? { cut: true } : {}),
    }
  )
}

/**
 * Lines an edit added and removed, counting each line of one side the other
 * side does not also hold. A moved line counts as neither.
 */
export function lineDelta(diff: StepDiff): { added: number; removed: number } {
  // A file's closing newline ends its last line; it does not start another.
  const lines = (text: string) => (text === "" ? [] : text.replace(/\n$/, "").split("\n"))
  const before = new Map<string, number>()
  for (const line of lines(diff.from)) before.set(line, (before.get(line) ?? 0) + 1)
  let added = 0
  for (const line of lines(diff.to)) {
    const left = before.get(line) ?? 0
    if (left > 0) before.set(line, left - 1)
    else added++
  }
  let removed = 0
  for (const left of before.values()) removed += left
  return { added, removed }
}

/**
 * One tool call as a step of its message. Times cross from the gateway's
 * clock to this device's by the view's own `serverNowMs`, as a usage limit's
 * reset does, so a running call's bar and the device's clock agree.
 */
export function toolStep(
  tool: ConversationTool,
  clock: { turnEnded: boolean; serverNowMs: number; receivedAtMs: number },
): StepPart {
  const onClient = (ms: number | undefined) =>
    ms === undefined
      ? undefined
      : clientDueAtMs(ms, clock.serverNowMs, clock.receivedAtMs)
  const startedAtMs = onClient(tool.startedAtMs)
  const endedAtMs = onClient(tool.endedAtMs)
  const changes = tool.changes ?? []
  const diff = stepDiff(changes)
  // Lines added and removed, across every file; unknown once a side was cut
  // or the list leaves out files the call changed.
  const delta =
    tool.changesCut || changes.some((change) => change.cut)
      ? {}
      : changes
          .map((change) =>
            lineDelta({
              filename: change.path,
              from: change.before ?? "",
              to: change.after,
            }),
          )
          .reduce(
            (sum, each) => ({
              added: sum.added + each.added,
              removed: sum.removed + each.removed,
            }),
            {
              added: 0,
              removed: 0,
            },
          )
  return {
    kind: "step",
    step: stepKind(tool.kind),
    label: tool.title,
    id: tool.toolId,
    status: stepStatus(tool.status, clock.turnEnded),
    ...(startedAtMs !== undefined ? { startedAtMs } : {}),
    ...(endedAtMs !== undefined ? { endedAtMs } : {}),
    ...(tool.input ? { input: tool.input } : {}),
    // A call that changed files shows the change, and its details stay its
    // output: they hold whatever else it reported beside the change (a
    // warning, a summary), which the change alone would lose.
    ...(diff ? { diff, ...delta } : {}),
    ...(tool.details ? { output: tool.details } : {}),
  }
}
