/**
 * What the in-memory source's agents say: a reply to a message, the outcome
 * of an approved command, and the answer to a refused one. Words only; the
 * source decides when they arrive.
 */
import type { ApprovalScope } from "../../application/ports"
import type { Part } from "../../model/transcript"

/** When each beat of a scripted answer arrives, in milliseconds. */
export const scriptTiming = {
  /** "Thinking" gives way to "Reading the workspace". */
  readingMs: 900,
  /** The reply starts to stream. */
  answerMs: 1600,
  /** Between each few words of a streamed reply. */
  stepMs: 38,
  wordsPerStep: 2,
  /** An approved command's run. */
  commandMs: 2400,
} as const

/**
 * What the source says is going on in a session at each beat of its script
 * (`SessionSummary.now`): one line, the way a person would say it.
 */
export interface ScriptedNow {
  /** Just sent: the agent is thinking about it. */
  readonly thinking: string
  /** Looking around the channel. */
  readonly reading: string
  /** Its reply streaming in. */
  readonly writing: string
}

/** The reply to a message: a look around the channel, then a short plan. */
export function replyTo(
  text: string,
  channel: string,
): { steps: readonly Part[]; text: string; now: ScriptedNow } {
  const ask = text.trim().replace(/[.?!]+$/, "")
  const topic =
    ask.length > 60 ? "this" : `“${ask.charAt(0).toLowerCase()}${ask.slice(1)}”`
  return {
    now: {
      thinking: `Thinking about ${topic}`,
      reading: `Reading what #${channel} already has around ${topic}`,
      writing: `Writing a short plan for ${topic}`,
    },
    steps: [
      {
        kind: "step",
        step: "search",
        label: "Searched the workspace",
        detail: "4 results",
      },
    ],
    text: `On it. I’ll start by reading what #${channel} already has around ${topic}, then come back with a short plan before I change anything. If it touches more than a couple of files, I’ll ask first.`,
  }
}

/** What is going on while an allowed command runs. */
export function runningNow(command: string): string {
  return `Running ${command.split(" ").slice(0, 2).join(" ")}, as you allowed`
}

export function approvedReply(
  command: string,
  scope: ApprovalScope,
): { parts: readonly Part[]; preview: string } {
  const preview =
    scope === "always"
      ? "That passed cleanly. I won’t ask before running it again."
      : "That passed cleanly. The change is ready for review whenever you are."
  return {
    parts: [
      { kind: "step", step: "run", label: "Ran", detail: command },
      { kind: "text", text: preview },
    ],
    preview,
  }
}

export function deniedReply(command: string): {
  parts: readonly Part[]
  preview: string
} {
  const preview = `Okay — I won’t run it. Everything else is in place; you can run it yourself with \`${command}\`.`
  return { parts: [{ kind: "text", text: preview }], preview }
}
