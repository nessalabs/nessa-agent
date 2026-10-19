/** A model request is an intent, never the source's confirmed selection. */
import type { ModelRef } from "./workspace-index"
import type { WorkspaceFailureReason } from "./failure"

export type ModelEffect = "subsequentResponses" | "nextTurn"
export interface ModelReceipt {
  readonly requestId: string
  readonly model: string
  readonly approvalMode: "ask" | "auto" | "full"
  readonly effect: ModelEffect
  /** A post-control read and actual source frame settled configuration, not necessarily this target. */
  readonly sourceReconciled: boolean
}
export type ModelChoice = {
  readonly requestId: string
  readonly model: ModelRef
} & (
  | { readonly state: "asking" | "acknowledged" }
  | {
      readonly state: "refused" | "recovery_required"
      readonly reason: WorkspaceFailureReason
      readonly code?: string
    }
)
export interface ModelControl {
  readonly pending?: {
    readonly requestId: string
    readonly requestedModel: string
    readonly status: "changing" | "recovery_required"
  }
  readonly effect?: ModelEffect
}

/** A receipt settles only after the independent source frame agrees. */
export function modelConfirmed(
  choice: ModelChoice,
  confirmed: ModelRef | undefined,
  pending: ModelControl["pending"],
): boolean {
  return (
    (choice.state === "acknowledged" || choice.state === "recovery_required") &&
    !pending &&
    confirmed?.provider === choice.model.provider &&
    confirmed.modelId === choice.model.modelId
  )
}
