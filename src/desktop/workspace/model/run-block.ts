/**
 * A shell fence in a reply as something the person can run
 * (`docs/design/terminals.md`, Run blocks). Pure: what a fence runs, how a
 * block names the terminals it started, and how a finished run reads.
 */
import type { TerminalEntry } from "./terminal"

/** The fence words that run as a shell. */
const shellWords = new Set(["bash", "sh", "shell", "zsh", "console"])

/** The most a Run sends, in bytes: the gateway's own cap on a command. */
export const runLimitBytes = 8 * 1024

export function runnableFence(language: string | undefined): boolean {
  return language !== undefined && shellWords.has(language.toLowerCase())
}

/**
 * What a fence runs: the code itself, or for a console fence only its
 * `$ ` lines (the rest is output it shows). Refused when nothing is left
 * or it is over `runLimitBytes`.
 */
export function runCommand(
  code: string,
  language: string | undefined,
): { readonly command: string } | { readonly refused: "empty" | "too-long" } {
  const command =
    language?.toLowerCase() === "console"
      ? code
          .split("\n")
          .filter((line) => line.startsWith("$ "))
          .map((line) => line.slice(2))
          .join("\n")
      : code.replace(/\s+$/, "")
  if (!command.trim()) return { refused: "empty" }
  if (new TextEncoder().encode(command).length > runLimitBytes)
    return { refused: "too-long" }
  return { command }
}

/**
 * The open request id a block's runs share before their attempt number:
 * the message, the part and the fence's place in it, so a block finds its
 * runs again after a reload and never another block's.
 */
export function runRequestBase(messageId: string, part: number, at: string): string {
  return `run ${messageId.slice(0, 200)} ${part} ${at.slice(0, 40)}`
}

/** The newest run a block started, if the list still holds one. */
export function latestRun(
  terminals: Readonly<Record<string, TerminalEntry>>,
  conversationId: string,
  base: string,
): TerminalEntry | undefined {
  let latest: TerminalEntry | undefined
  for (const terminal of Object.values(terminals)) {
    if (terminal.kind !== "run" || terminal.conversationId !== conversationId) continue
    if (!terminal.requestId.startsWith(`${base} #`)) continue
    if (!latest || terminal.startedAtMs > latest.startedAtMs) latest = terminal
  }
  return latest
}

/** The request id of a block's next run. */
export function runRequestId(base: string, attempt: string): string {
  return `${base} #${attempt}`
}

/** How long something took, as a person reads it: 0.4s, 12s, 3m 5s. */
export function durationWords(ms: number): string {
  const seconds = Math.max(0, ms) / 1000
  if (seconds < 10) return `${seconds.toFixed(1)}s`
  if (seconds < 60) return `${Math.round(seconds)}s`
  const minutes = Math.floor(seconds / 60)
  return `${minutes}m ${Math.round(seconds - minutes * 60)}s`
}

/** A finished run's line: `exit 0 · 1.2s`. Nothing while it runs. */
export function runEndWords(terminal: TerminalEntry): string | undefined {
  if (terminal.state !== "exited") return undefined
  const status =
    terminal.exitStatus === undefined ? "ended" : `exit ${terminal.exitStatus}`
  if (terminal.endedAtMs === undefined) return status
  return `${status} · ${durationWords(terminal.endedAtMs - terminal.startedAtMs)}`
}
