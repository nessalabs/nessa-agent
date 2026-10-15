import * as React from "react"
import {
  AgentApiKeySaveUncertain,
  type AgentApiKeySink,
  type ApiKeyAgent,
} from "../application/ports"

/** Retains audit failure independently of the readiness-controlled key form. */
export function useAgentApiKeySave(sink: AgentApiKeySink | undefined) {
  const [auditFailure, setAuditFailure] = React.useState<"saved" | "uncertain">()
  async function save(agent: ApiKeyAgent, key: string) {
    if (!sink) throw new Error("agent API-key storage is unavailable")
    try {
      const saved = await sink.save(agent, key)
      if (saved.status === "saved-audit-failed") setAuditFailure("saved")
      return saved
    } catch (failure) {
      if (failure instanceof AgentApiKeySaveUncertain && failure.auditStatus === "failed")
        setAuditFailure("uncertain")
      throw failure
    }
  }
  return { save, auditFailure }
}

export function AgentApiKeyAuditWarning({
  failure,
}: {
  failure: "saved" | "uncertain" | undefined
}) {
  return failure ? (
    <p role="alert" className="nessa-text-2 text-destructive">
      {failure === "saved"
        ? "The key was saved, but Nessa could not record its security audit record."
        : "Nessa could not confirm the key save or record its security audit outcome."}
    </p>
  ) : null
}
