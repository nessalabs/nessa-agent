import copies from "./startup-refusals.json"

/**
 * What startup can say, in one file (`startup-refusals.json`).
 *
 * `line` is the sentence a person reads. `code` is the stable token under
 * it. The other strings are the log: the host, `pnpm app`, and the page's
 * own console. The desktop host reads this same file.
 */
export type StartupRefusalReason = "not-provisioned" | "not-ready" | "wrong-stage"

/** Sentences read by name. A stage mismatch is filled, not looked up. */
export type NamedStartupSentence = "not-provisioned" | "not-ready" | "not-listening"

/** Keys of `code` in the shared file. Each one is a `STARTUP_` token. */
export type StartupCodeKey =
  | "not-provisioned"
  | "not-ready"
  | "not-listening"
  | "wrong-stage"
  | "document-unserved"
  | "script-unserved"
  | "still-compiling"
  | "runtime"
  | "host"

function owned(value: object, key: string): unknown {
  if (!Object.hasOwn(value, key)) return undefined
  return (value as Record<string, unknown>)[key]
}

export type StageMismatch = {
  readonly bundle: string
  readonly requested: string
}

function sentence(key: string): string {
  const value = owned(copies, key)
  if (typeof value !== "string" || value.length === 0)
    throw new Error(`startup sentence ${key} is not text`)
  return value
}

/** The one sentence a person reads on the startup screen. */
export function startupLine(): string {
  return sentence("line")
}

/** The copyable token for `key`. The log sentence stays off the screen. */
export function startupCode(key: StartupCodeKey): string {
  const table = owned(copies, "code")
  if (typeof table !== "object" || table === null)
    throw new Error("startup codes are missing")
  const value = owned(table, key)
  if (typeof value !== "string" || !/^STARTUP_[A-Z]+$/.test(value))
    throw new Error(`startup code ${key} is not a code`)
  return value
}

function fill(key: string, values: Readonly<Record<string, string>>): string {
  return Object.entries(values).reduce(
    (text, [name, value]) => text.replaceAll(`{${name}}`, value),
    sentence(key),
  )
}

export function startupRefusalSentence(reason: NamedStartupSentence): string {
  return sentence(reason)
}

export function wrongStageSentence(stages: StageMismatch | null | undefined): string {
  if (!stages) return sentence("wrong-stage-plain")
  return fill("wrong-stage", stages)
}

export function documentUnservedSentence(page: string): string {
  return fill("document-unserved", { page })
}

export function scriptUnservedSentence(page: string, script: string): string {
  return fill("script-unserved", { page, script })
}

export function stillCompilingSentence(page: string, script: string): string {
  return fill("still-compiling", { page, script })
}

/** A host command refused with a reason the page can branch on, not a sentence it parses. */
export class HostRefusalError extends Error {
  readonly reason: StartupRefusalReason
  readonly bundle?: string
  readonly requested?: string

  constructor(reason: StartupRefusalReason, stages?: StageMismatch) {
    super(
      reason === "wrong-stage"
        ? wrongStageSentence(stages)
        : startupRefusalSentence(reason),
    )
    this.name = "HostRefusalError"
    this.reason = reason
    this.bundle = stages?.bundle
    this.requested = stages?.requested
  }
}

function textField(value: object, key: string): string | undefined {
  if (!Object.hasOwn(value, key)) return undefined
  const field = (value as Record<string, unknown>)[key]
  return typeof field === "string" && field.length > 0 ? field : undefined
}

/**
 * The invoke rejection the host serializes for a startup refusal.
 *
 * Anything else — a string, an Error, an object without one of these reasons —
 * is not this, so a payload the host did not mean stays off the page.
 */
export function hostRefusalFromInvoke(value: unknown): HostRefusalError | undefined {
  if (typeof value !== "object" || value === null) return undefined
  const reason = textField(value, "reason")
  if (reason === "not-provisioned" || reason === "not-ready")
    return new HostRefusalError(reason)
  if (reason !== "wrong-stage") return undefined
  const bundle = textField(value, "bundle")
  const requested = textField(value, "requested")
  if (!bundle || !requested) return undefined
  return new HostRefusalError("wrong-stage", { bundle, requested })
}
