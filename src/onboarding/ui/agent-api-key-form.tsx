import * as React from "react"

import { Button } from "@nessa-ui/react/button"
import type { ApiKeyAgent } from "../application/ports"

/** Reusable API-key entry for an explicitly supported local agent.
 *
 * The callback is injected by composition. This component never persists,
 * logs, or includes the key in status text. A successful write clears the
 * field before asking setup to refresh readiness from the gateway. */
export function AgentApiKeyForm({
  agent,
  agentName,
  disabled = false,
  onSave,
  onSaved,
}: {
  agent: ApiKeyAgent
  agentName: string
  disabled?: boolean
  onSave: (agent: ApiKeyAgent, key: string) => Promise<void>
  onSaved: () => void | Promise<void>
}) {
  const [key, setKey] = React.useState("")
  const [saving, setSaving] = React.useState(false)
  const [outcome, setOutcome] = React.useState<"saved" | "failed" | "refresh-failed">()

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    setOutcome(undefined)
    try {
      await onSave(agent, key)
    } catch {
      setOutcome("failed")
      setSaving(false)
      return
    }
    setKey("")
    setOutcome("saved")
    try {
      await onSaved()
    } catch {
      setOutcome("refresh-failed")
    } finally {
      setSaving(false)
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-2 rounded-xl border p-3">
      <label htmlFor={`${agent}-api-key`} className="nessa-text-2 font-medium">
        {agentName} API key
      </label>
      <input
        id={`${agent}-api-key`}
        name="apiKey"
        type="password"
        autoComplete="off"
        required
        maxLength={16384}
        disabled={disabled || saving}
        spellCheck={false}
        value={key}
        onChange={(event) => setKey(event.currentTarget.value)}
        className="rounded-lg border bg-background px-3 py-2"
      />
      <Button
        type="submit"
        variant="outline"
        disabled={disabled || saving || key.length === 0}
      >
        {saving ? "Saving…" : "Save key"}
      </Button>
      {outcome === "saved" ? (
        <p
          role="status"
          aria-live="polite"
          className="nessa-text-2 text-muted-foreground"
        >
          Key saved. Checking {agentName} again…
        </p>
      ) : null}
      {outcome === "failed" ? (
        <p role="alert" className="nessa-text-2 text-destructive">
          Nessa could not save this key. Try again.
        </p>
      ) : null}
      {outcome === "refresh-failed" ? (
        <p role="alert" className="nessa-text-2 text-destructive">
          Key saved, but Nessa could not check {agentName} again. Retry the check.
        </p>
      ) : null}
    </form>
  )
}
