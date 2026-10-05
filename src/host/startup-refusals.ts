import copies from "./startup-refusals.json"

/**
 * The sentences a window shows when startup cannot reach the local server.
 *
 * One file owns them (`startup-refusals.json`). The desktop host reads that
 * same file for the log line and the page it shows when the document never
 * arrives; this module is what the running page says.
 */
const sentences: Readonly<Record<string, string>> = copies

export type StartupRefusalReason = "not-provisioned" | "not-ready" | "wrong-stage"

export type StageMismatch = {
  readonly bundle: string
  readonly requested: string
}

function sentence(key: string): string {
  if (!Object.hasOwn(sentences, key)) throw new Error(`missing startup sentence ${key}`)
  const value = sentences[key]
  if (typeof value !== "string" || value.length === 0)
    throw new Error(`startup sentence ${key} is not text`)
  return value
}

function fill(key: string, values: Readonly<Record<string, string>>): string {
  return Object.entries(values).reduce(
    (text, [name, value]) => text.replaceAll(`{${name}}`, value),
    sentence(key),
  )
}

export function startupRefusalSentence(
  reason: Exclude<StartupRefusalReason, "wrong-stage">,
): string {
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
  const field = value[key as keyof typeof value]
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
  if (!Object.hasOwn(value, "reason")) return undefined
  const reason = value.reason
  if (reason === "not-provisioned" || reason === "not-ready")
    return new HostRefusalError(reason)
  if (reason !== "wrong-stage") return undefined
  const bundle = textField(value, "bundle")
  const requested = textField(value, "requested")
  if (!bundle || !requested) return undefined
  return new HostRefusalError("wrong-stage", { bundle, requested })
}
