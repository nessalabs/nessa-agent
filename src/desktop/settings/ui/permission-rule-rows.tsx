import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react"
import type { PermissionRule } from "@nessa/client"
import type { PermissionRulesGateway } from "../adapters/permission-rules-gateway"
import { setting } from "../model/settings-catalogue"
import { Group, ItemRow } from "./settings-controls"

/**
 * Settings › Privacy & Permissions › Access: the owner's remembered rules,
 * each one tool in one project, and a way to revoke one. With no gateway the
 * row rests. Another client can revoke a rule, so the list is read again
 * when the window regains focus or becomes visible.
 */
const PermissionRulesContext = createContext<PermissionRulesGateway | undefined>(
  undefined,
)

export function PermissionRulesProvider({
  gateway,
  children,
}: {
  gateway: PermissionRulesGateway | undefined
  children: ReactNode
}) {
  return (
    <PermissionRulesContext.Provider value={gateway}>
      {children}
    </PermissionRulesContext.Provider>
  )
}

type Phase =
  | { readonly kind: "absent" }
  | { readonly kind: "reading" }
  | { readonly kind: "unread" }
  | {
      readonly kind: "ready"
      readonly rules: readonly PermissionRule[]
      readonly revoking?: string
      readonly failed?: boolean
    }

export function PermissionRuleRows() {
  const gateway = useContext(PermissionRulesContext)
  const [phase, setPhase] = useState<Phase>({ kind: "absent" })
  const latest = useRef(0)
  useEffect(() => {
    if (!gateway) return
    let current = true
    const read = () => {
      const generation = ++latest.current
      void gateway.read().then((outcome) => {
        if (!current || generation !== latest.current) return
        setPhase(
          outcome.ok ? { kind: "ready", rules: outcome.rules } : { kind: "unread" },
        )
      })
    }
    setPhase({ kind: "reading" })
    read()
    const onFocus = () => read()
    const onVisible = () => {
      if (document.visibilityState === "visible") read()
    }
    window.addEventListener("focus", onFocus)
    document.addEventListener("visibilitychange", onVisible)
    return () => {
      current = false
      window.removeEventListener("focus", onFocus)
      document.removeEventListener("visibilitychange", onVisible)
    }
  }, [gateway])
  const revoke = (ruleId: string) => {
    if (!gateway || phase.kind !== "ready" || phase.revoking) return
    const generation = ++latest.current
    setPhase({ kind: "ready", rules: phase.rules, revoking: ruleId })
    void gateway.revoke(ruleId, crypto.randomUUID()).then(async (outcome) => {
      const again = await gateway.read()
      if (generation !== latest.current) return
      if (!outcome.ok) {
        setPhase({
          kind: "ready",
          rules: again.ok ? again.rules : phase.rules,
          failed: true,
        })
        return
      }
      if (again.ok) setPhase({ kind: "ready", rules: again.rules })
      else
        setPhase({
          kind: "ready",
          rules: phase.rules.filter((rule) => rule.id !== ruleId || !outcome.applied),
        })
    })
  }
  const entry = setting("remembered-rules")
  const resting =
    phase.kind === "absent"
      ? "Not available in this window."
      : phase.kind === "reading"
        ? "Reading remembered rules."
        : phase.kind === "unread"
          ? "Could not read remembered rules."
          : undefined
  return (
    <Group title={entry.label} note={entry.detail}>
      {resting !== undefined ? (
        <ItemRow
          setting="remembered-rules"
          label={entry.label}
          detail={resting}
          unavailable
        />
      ) : null}
      {phase.kind === "ready" && phase.failed ? (
        <ItemRow label="Could not revoke that rule." detail="It is still remembered." />
      ) : null}
      {phase.kind === "ready" && phase.rules.length === 0 ? (
        <ItemRow
          setting="remembered-rules"
          label={entry.label}
          detail="Rules you save from a review appear here."
        />
      ) : null}
      {phase.kind === "ready"
        ? phase.rules.map((rule, index) => (
            <ItemRow
              key={rule.id}
              setting={index === 0 ? "remembered-rules" : undefined}
              label={`${rule.effect === "allow" ? "Always allow" : "Always deny"} ${rule.tool}`}
              detail={`${rule.source} · ${rule.workspace}`}
              data-rule={rule.id}
              control={
                <button
                  type="button"
                  disabled={phase.revoking !== undefined}
                  onClick={() => revoke(rule.id)}
                >
                  Revoke
                </button>
              }
            />
          ))
        : null}
    </Group>
  )
}
