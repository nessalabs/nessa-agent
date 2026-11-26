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

/** The rule to send, or why it cannot be. */
export function parseRule(draft: RuleDraft): { rule: ResumeRule } | { problem: string } {
  const program = draft.program.trim()
  if (!program) return { problem: "Name a program, such as claude, or its full path." }
  if (program.includes("/") && !program.startsWith("/"))
    return { problem: "A path starts with /, such as /opt/homebrew/bin/nvim." }
  const folder = draft.folder.trim()
  if (folder && !folder.startsWith("/"))
    return { problem: "A folder starts with /, such as /Users/you/code." }
  const args = draft.args.split(/\s+/).filter(Boolean)
  if (args.length > 16) return { problem: "At most 16 arguments." }
  return {
    rule: {
      program,
      action: draft.action,
      ...(folder ? { folder } : {}),
      ...(args.length ? { args } : {}),
    },
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
