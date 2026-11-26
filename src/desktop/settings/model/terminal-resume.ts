/**
 * Settings › Terminal › Resume in words: a program rule as typed and as
 * read, and a kept program's line. The gateway checks every rule again; this
 * only stops a rule it would refuse before it is sent.
 */

/** One program rule: which program, where, with what, and Allow or Never. */
export interface ResumeRule {
  /** `*`, a file name, or an absolute path. */
  readonly program: string
  readonly action: "allow" | "never"
  readonly folder?: string
  readonly args?: string[]
}

/** A program kept now, and whether it comes back at the next restart. */
export interface KeptProgram {
  readonly command: string
  readonly comesBack: boolean
}

/** A rule as the person types it. */
export interface RuleDraft {
  readonly program: string
  readonly folder: string
  /** Arguments the command must hold, separated by spaces. */
  readonly args: string
  readonly action: "allow" | "never"
}

/** The rule to send: the draft trimmed, its arguments split. The gateway
 * decides whether it is acceptable; see {@link RuleRefused}. */
export function draftRule(draft: RuleDraft): ResumeRule {
  const folder = draft.folder.trim()
  const args = draft.args.split(/\s+/).filter(Boolean)
  return {
    program: draft.program.trim(),
    action: draft.action,
    ...(folder ? { folder } : {}),
    ...(args.length ? { args } : {}),
  }
}

/** The part of a rule the gateway refused, or `count` for too many rules. */
export type RefusedPart = "program" | "folder" | "args" | "count"

/** The gateway refused the rules it was sent and kept the ones it had. */
export class RuleRefused extends Error {
  constructor(readonly part: RefusedPart) {
    super(`Terminal resume rule refused (${part})`)
    this.name = "RuleRefused"
  }
}

/** What to fix, for a refused part. */
export function refusalMessage(part: RefusedPart): string {
  switch (part) {
    case "program":
      return "Name a program, such as claude, or its full path starting with /."
    case "folder":
      return "A folder starts with /, such as /Users/you/code."
    case "args":
      return "Too many arguments, or one is too long."
    case "count":
      return "That is as many rules as Nessa keeps. Remove one first."
  }
}

/** A rule's line under its program: "Allow · in /work · with --resume". */
export function describeRule(rule: ResumeRule): string {
  const parts: string[] = [rule.action === "never" ? "Never" : "Allow"]
  if (rule.program === "*") parts[0] += " every program"
  if (rule.folder) parts.push(`in ${rule.folder}`)
  if (rule.args?.length) parts.push(`with ${rule.args.join(" ")}`)
  return parts.join(" · ")
}

/** A kept program's line: its command, and whether it comes back. */
export function savedProgramDetail(saved: KeptProgram): string {
  const fate = saved.comesBack
    ? "Comes back after a restart"
    : "Won’t come back: no rule allows it, or resume is off"
  return `${saved.command} · ${fate}`
}
