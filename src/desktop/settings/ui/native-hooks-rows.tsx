import { createContext, useContext, useEffect, useState, type ReactNode } from "react"
import { setting, type SettingId } from "../model/settings-catalogue"
import type {
  NativeHooksAgentName,
  NativeHooksChoice,
  NativeHooksGateway,
} from "../adapters/native-hooks-gateway"
import { Group, ItemRow, Toggle } from "./settings-controls"

/**
 * Settings › Privacy & Permissions › Access: whether the next launch of
 * Claude or Codex omits Nessa's request to leave that harness's own hooks
 * off. With no gateway the rows rest. OpenCode has no row.
 */
const NativeHooksContext = createContext<NativeHooksGateway | undefined>(undefined)

export function NativeHooksProvider({
  gateway,
  children,
}: {
  gateway: NativeHooksGateway | undefined
  children: ReactNode
}) {
  return (
    <NativeHooksContext.Provider value={gateway}>{children}</NativeHooksContext.Provider>
  )
}

const warning =
  "Hooks can approve, deny, or change a tool call and its result without Nessa asking, and can run commands when the agent starts."

const rows = [
  { id: "claude-native-hooks", agent: "claude" },
  { id: "codex-native-hooks", agent: "codex" },
] as const satisfies readonly { id: SettingId; agent: NativeHooksAgentName }[]

type Phase =
  | { readonly kind: "absent" }
  | { readonly kind: "reading" }
  | { readonly kind: "unread" }
  | {
      readonly kind: "ready"
      readonly choice: NativeHooksChoice
      readonly saving?: NativeHooksAgentName
      readonly failed?: NativeHooksAgentName
    }

export function NativeHooksRows() {
  const gateway = useContext(NativeHooksContext)
  const [phase, setPhase] = useState<Phase>({ kind: "absent" })
  useEffect(() => {
    if (!gateway) return
    let current = true
    setPhase({ kind: "reading" })
    void gateway.read().then((outcome) => {
      if (!current) return
      setPhase(
        outcome.ok ? { kind: "ready", choice: outcome.choice } : { kind: "unread" },
      )
    })
    return () => {
      current = false
    }
  }, [gateway])
  const set = (agent: NativeHooksAgentName, enabled: boolean) => {
    if (!gateway || phase.kind !== "ready" || phase.saving) return
    const previous = phase.choice
    setPhase({ kind: "ready", choice: { ...previous, [agent]: enabled }, saving: agent })
    void gateway.set(agent, enabled, crypto.randomUUID()).then(async (outcome) => {
      const again = await gateway.read()
      if (outcome.ok && outcome.agent === agent) {
        setPhase(
          again.ok
            ? { kind: "ready", choice: again.choice }
            : {
                kind: "ready",
                choice: { ...previous, [outcome.agent]: outcome.enabled },
              },
        )
        return
      }
      // A lost response may still have been applied. The read is what is in effect.
      setPhase(
        again.ok
          ? { kind: "ready", choice: again.choice, failed: agent }
          : { kind: "unread" },
      )
    })
  }
  return (
    <Group title="Native hooks" note={warning}>
      {rows.map((row) => (
        <HookRow
          key={row.id}
          id={row.id}
          agent={row.agent}
          phase={gateway ? phase : { kind: "absent" }}
          onChange={(enabled) => set(row.agent, enabled)}
        />
      ))}
    </Group>
  )
}

function HookRow({
  id,
  agent,
  phase,
  onChange,
}: {
  id: SettingId
  agent: NativeHooksAgentName
  phase: Phase
  onChange: (enabled: boolean) => void
}) {
  const entry = setting(id)
  if (phase.kind !== "ready") {
    const detail =
      phase.kind === "absent"
        ? "Not available in this window."
        : phase.kind === "reading"
          ? "Reading this choice."
          : "Could not read this choice."
    return <ItemRow setting={id} label={entry.label} detail={detail} unavailable />
  }
  const failed = phase.failed === agent
  const configured =
    agent === "claude"
      ? phase.choice.claudeConfigured !== false
      : phase.choice.codexConfigured !== false
  const detail = failed
    ? "Could not change this."
    : configured
      ? entry.detail
      : `${entry.detail} This agent isn't set up.`
  return (
    <ItemRow
      setting={id}
      label={entry.label}
      detail={detail}
      control={
        <Toggle
          checked={phase.choice[agent]}
          disabled={phase.saving !== undefined}
          onChange={onChange}
        />
      }
    />
  )
}
