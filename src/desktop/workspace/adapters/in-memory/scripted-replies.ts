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

/** The reply to a message: a look around the channel, then a short plan. */
export function replyTo(
  text: string,
  channel: string,
): { steps: readonly Part[]; text: string } {
  const ask = text.trim().replace(/[.?!]+$/, "")
  const topic =
    ask.length > 60 ? "this" : `“${ask.charAt(0).toLowerCase()}${ask.slice(1)}”`
  return {
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
