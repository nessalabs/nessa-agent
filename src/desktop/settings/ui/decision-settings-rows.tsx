import { Button } from "@nessa-ui/react/button"
import { Input } from "@nessa-ui/react/input"
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import type { DecisionSettings } from "@nessa/client"
import type { DecisionSettingsGateway } from "../adapters/decision-settings-gateway"
import { setting } from "../model/settings-catalogue"
import { useRefreshDecisions } from "../../workspace/ui/chrome/decisions-source"
import { Choice, Group, ItemRow, Toggle } from "./settings-controls"

/**
 * Settings › Chat settings › Decisions: whether an ask the agent allowed it
 * on is answered with its recommended choice when nobody answers in time,
 * and after how long. The gateway keeps it, for this person, and picks; this
 * only reads and saves it. With no gateway the rows rest.
 */
const DecisionSettingsContext = createContext<DecisionSettingsGateway | undefined>(
  undefined,
)

export function DecisionSettingsProvider({
  gateway,
  children,
}: {
  gateway: DecisionSettingsGateway | undefined
  children: ReactNode
}) {
  return (
    <DecisionSettingsContext.Provider value={gateway}>
      {children}
    </DecisionSettingsContext.Provider>
  )
}

/** The wait is a whole number of one of these; the gateway takes 60 s to a day. */
const units = {
  minutes: { seconds: 60, most: 1440 },
  hours: { seconds: 3600, most: 24 },
} as const
type Unit = keyof typeof units
const unitOptions = [
  { id: "minutes", label: "minutes" },
  { id: "hours", label: "hours" },
] as const

/** A wait in the largest unit it is a whole number of: 7200 → 2 hours. */
function split(seconds: number): { amount: number; unit: Unit } {
  return seconds % 3600 === 0
    ? { amount: seconds / 3600, unit: "hours" }
    : { amount: Math.round(seconds / 60), unit: "minutes" }
}

/** The typed amount in seconds, or why it cannot be saved. */
function parse(text: string, unit: Unit): { seconds: number } | { problem: string } {
  const trimmed = text.trim()
  const amount = /^\d+$/.test(trimmed) ? Number(trimmed) : Number.NaN
  const most = units[unit].most
  if (!Number.isSafeInteger(amount) || amount < 1 || amount > most)
    return { problem: `1 to ${most.toLocaleString("en-US")} ${unit}.` }
  return { seconds: amount * units[unit].seconds }
}

type Phase =
  | { readonly kind: "reading" }
  | { readonly kind: "unread" }
  | {
      readonly kind: "ready"
      readonly settings: DecisionSettings
      readonly saving: boolean
    }

export function DecisionSettingsRows() {
  const gateway = useContext(DecisionSettingsContext)
  const [phase, setPhase] = useState<Phase>({ kind: "reading" })
  const [amount, setAmount] = useState("")
  const [unit, setUnit] = useState<Unit>("minutes")
  const [problem, setProblem] = useState("")
  const [failed, setFailed] = useState(false)
  const [retry, setRetry] = useState(0)
  const problemId = useId()
  // Every countdown on screen follows these settings: read them again once saved.
  const refreshDecisions = useRefreshDecisions()
  // Only the newest read or save may show what it got.
  const revision = useRef(0)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const show = (settings: DecisionSettings) => {
    const wait = split(settings.autoPickAfterSeconds)
    setAmount(String(wait.amount))
    setUnit(wait.unit)
    setProblem("")
    setPhase({ kind: "ready", settings, saving: false })
  }
  useEffect(() => {
    if (!gateway) return
    const current = ++revision.current
    setPhase({ kind: "reading" })
    gateway.read().then(
      (settings) => {
        if (current === revision.current) show(settings)
      },
      () => {
        if (current === revision.current) setPhase({ kind: "unread" })
      },
    )
    return () => {
      revision.current += 1
    }
  }, [gateway, retry])
  const save = (next: DecisionSettings) => {
    if (!gateway || phase.kind !== "ready" || phase.saving) return
    const previous = phase.settings
    if (
      next.autoPick === previous.autoPick &&
      next.autoPickAfterSeconds === previous.autoPickAfterSeconds
    )
      return
    const current = ++revision.current
    setFailed(false)
    setPhase({ kind: "ready", settings: next, saving: true })
    gateway.set(next).then(
      (kept) => {
        refreshDecisions()
        if (current === revision.current) show(kept)
      },
      () => {
        refreshDecisions()
        if (current !== revision.current) return
        // A lost answer may still have been kept: read what is in effect.
        setFailed(true)
        setRetry((value) => value + 1)
      },
    )
  }
  const commit = (text: string, inUnit: Unit) => {
    if (phase.kind !== "ready") return
    const parsed = parse(text, inUnit)
    if ("problem" in parsed) {
      setProblem(parsed.problem)
      return
    }
    setProblem("")
    save({ ...phase.settings, autoPickAfterSeconds: parsed.seconds })
  }
  const ready = !!gateway && phase.kind === "ready"
  const settings = phase.kind === "ready" ? phase.settings : undefined
  const busy = phase.kind === "ready" && phase.saving
  const on = settings?.autoPick ?? false
  const status = !gateway
    ? "Connect a gateway"
    : phase.kind === "reading"
      ? "Loading…"
      : undefined
  const toggle = setting("auto-pick")
  const after = setting("auto-pick-after")
  return (
    <Group>
      <ItemRow
        label={toggle.label}
        detail={status ?? toggle.detail}
        setting="auto-pick"
        unavailable={!ready}
        data-auto-pick={settings ? String(settings.autoPick) : undefined}
        control={
          <Toggle
            checked={on}
            disabled={busy}
            onChange={(next) => settings && save({ ...settings, autoPick: next })}
          />
        }
      />
      <ItemRow
        label={after.label}
        setting="auto-pick-after"
        unavailable={!ready || !on}
        describedBy={problem ? problemId : undefined}
        data-auto-pick-after={
          settings ? String(settings.autoPickAfterSeconds) : undefined
        }
        control={
          // Keyed: the row maps a wrapper's children to name its controls.
          <span className="settings-decisions-wait">
            <Input
              key="amount"
              className="settings-decisions-amount"
              type="text"
              inputMode="numeric"
              aria-label={`${after.label}, in ${unit}`}
              aria-invalid={problem ? true : undefined}
              aria-describedby={problem ? problemId : undefined}
              data-auto-pick-amount=""
              disabled={!ready || !on}
              readOnly={busy}
              value={amount}
              onChange={(event) => {
                setAmount(event.target.value)
                if (problem) setProblem("")
              }}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return
                event.preventDefault()
                commit(amount, unit)
              }}
              onBlur={() => {
                if (mounted.current) commit(amount, unit)
              }}
            />
            <Choice
              key="unit"
              label={`${after.label}, unit`}
              value={unit}
              options={unitOptions}
              data-auto-pick-unit=""
              onChange={(next) => {
                setUnit(next)
                commit(amount, next)
              }}
            />
          </span>
        }
      />
      {problem && (
        <p id={problemId} role="alert" className="settings-boat-note">
          {problem}
        </p>
      )}
      {(phase.kind === "unread" || failed) && gateway && (
        <p role="alert" className="settings-boat-note">
          {failed ? "Couldn’t confirm the change." : "Couldn’t read this."}{" "}
          {phase.kind === "unread" && (
            <Button
              className="settings-button"
              type="button"
              onClick={() => setRetry((value) => value + 1)}
            >
              Reload
            </Button>
          )}
        </p>
      )}
    </Group>
  )
}
