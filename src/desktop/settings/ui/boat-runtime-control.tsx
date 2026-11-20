import { Button } from "@nessa-ui/react/button"
import { Input } from "@nessa-ui/react/input"
import { useEffect, useId, useRef, useState } from "react"
import {
  vmLifetimeSeconds,
  type BoatRuntimeSetParams,
  type BoatRuntimeSettings,
  type BoatVmLifetime,
} from "@nessa/client"
import type { BoatGateway } from "../adapters/boat-gateway"
import type { ChoiceOption } from "../../ui/choice-menu"
import { Choice, ItemRow } from "./settings-controls"

/**
 * Auto-stop is any whole number of a unit, or Never, or (for a channel) the
 * default. The protocol's `vmLifetimeSeconds` owns which lengths can be
 * saved; this control only turns an amount and a unit into seconds.
 */
const units = {
  minutes: { seconds: 60, one: "minute", many: "minutes" },
  hours: { seconds: 3600, one: "hour", many: "hours" },
  days: { seconds: 86_400, one: "day", many: "days" },
  // Offered only for a saved length that is not whole minutes (set elsewhere).
  seconds: { seconds: 1, one: "second", many: "seconds" },
} as const
type Unit = keyof typeof units
const offered: readonly Unit[] = ["minutes", "hours", "days"]
type Mode = "default" | "never"
type Choosing = Mode | Unit
type Saving = Pick<BoatRuntimeSetParams, "mode" | "seconds">
type Length = { amount: number; unit: Unit }

/** The largest unit a length is a whole number of: 7200 → 2 hours. */
function split(seconds: number): Length {
  for (const unit of ["days", "hours", "minutes"] as const)
    if (seconds % units[unit].seconds === 0)
      return { amount: seconds / units[unit].seconds, unit }
  return { amount: seconds, unit: "seconds" }
}
const named = (amount: number, unit: Unit) =>
  `${amount.toLocaleString("en-US")} ${amount === 1 ? units[unit].one : units[unit].many}`
function label(lifetime: BoatVmLifetime): string {
  if (lifetime.mode === "never") return "never"
  if (lifetime.seconds === undefined) return "unknown"
  const { amount, unit } = split(lifetime.seconds)
  return named(amount, unit)
}
function key(lifetime: BoatVmLifetime | undefined): string {
  return lifetime === undefined
    ? "default"
    : lifetime.mode === "never"
      ? "never"
      : String(lifetime.seconds)
}
/** The most of a unit that can be saved: 49,710 days. */
const most = (unit: Unit) => Math.floor(vmLifetimeSeconds.maximum / units[unit].seconds)
/** The typed amount in seconds, or why it cannot be saved. */
function parse(text: string, unit: Unit): { seconds: number } | { problem: string } {
  const trimmed = text.trim()
  const amount = /^\d+$/.test(trimmed) ? Number(trimmed) : Number.NaN
  if (!Number.isSafeInteger(amount) || amount < 1)
    return { problem: `Enter a whole number of ${units[unit].many}, 1 or more.` }
  if (amount > most(unit)) return { problem: `Up to ${named(most(unit), unit)}.` }
  return { seconds: amount * units[unit].seconds }
}

/** Default and channel editors share one control and ordinary gateway config. */
export function BoatRuntimeControl({
  channelId,
  gateway,
}: {
  channelId?: string
  gateway?: Pick<BoatGateway, "runtimeSettings" | "setRuntimeSettings">
}) {
  const revision = useRef(0)
  const savePending = useRef(false)
  // A choice made while another saves (an amount saved as its field loses
  // focus to the unit menu, then the unit) is saved after it, not dropped.
  const queued = useRef<{ choice: Saving; entered?: Length } | undefined>(undefined)
  // The saved choice's key, read by a queued save after its state is stale.
  const savedKey = useRef<string | undefined>(undefined)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const problemId = useId()
  const [settings, setSettings] = useState<BoatRuntimeSettings>()
  // What the control shows as chosen: "default", "never" or the seconds.
  const [selection, setSelection] = useState("")
  // The amount being typed and its unit, shown while auto-stop is a length.
  const [amount, setAmount] = useState("")
  const [unit, setUnit] = useState<Unit>("hours")
  const [problem, setProblem] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [retry, setRetry] = useState(0)
  const own = (value: BoatRuntimeSettings) =>
    channelId === undefined ? value.defaultLifetime : value.channelOverride
  /**
   * Show what was read or saved, dropping anything half typed. A length saved
   * as it was entered keeps its words (24 hours stays 24 hours, not 1 day),
   * so a unit picked from a menu opened over it applies to that amount.
   */
  const show = (value: BoatRuntimeSettings, entered?: Length) => {
    const lifetime = own(value)
    savedKey.current = key(lifetime)
    setSelection(key(lifetime))
    setProblem("")
    if (lifetime?.mode === "after" && lifetime.seconds !== undefined) {
      const length =
        entered && entered.amount * units[entered.unit].seconds === lifetime.seconds
          ? entered
          : split(lifetime.seconds)
      setAmount(String(length.amount))
      setUnit(length.unit)
    }
  }
  useEffect(() => {
    const current = ++revision.current
    setSettings(undefined)
    setSelection("")
    setAmount("")
    setUnit("hours")
    setProblem("")
    setError("")
    setBusy(false)
    savePending.current = false
    queued.current = undefined
    savedKey.current = undefined
    void gateway
      ?.runtimeSettings(channelId === undefined ? {} : { channelId })
      .then((value) => {
        if (current !== revision.current) return
        setSettings(value)
        show(value)
      })
      .catch(() => {
        if (current === revision.current) setError("Couldn’t read this.")
      })
    return () => {
      revision.current += 1
    }
    // `show` reads only `channelId`, already a dependency.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [gateway, channelId, retry])
  const submit = async (choice: Saving, entered?: Length): Promise<void> => {
    const next = choice.mode === "after" ? String(choice.seconds) : choice.mode
    if (savePending.current) {
      queued.current = { choice, entered }
      setSelection(next)
      return
    }
    if (!gateway || savedKey.current === undefined || next === savedKey.current) return
    const current = ++revision.current
    savePending.current = true
    setSelection(next)
    setBusy(true)
    setError("")
    try {
      const value = await gateway.setRuntimeSettings({
        ...choice,
        ...(channelId === undefined ? {} : { channelId }),
      })
      if (current !== revision.current) return
      setSettings(value)
      show(value, entered)
    } catch {
      if (current === revision.current) {
        queued.current = undefined
        savedKey.current = undefined
        setError("Couldn’t confirm the change.")
        setSettings(undefined)
      }
    } finally {
      if (current === revision.current) {
        savePending.current = false
        setBusy(false)
        const after = queued.current
        queued.current = undefined
        if (after) void submit(after.choice, after.entered)
      }
    }
  }
  /** Save the typed amount in `inUnit`, or say why it cannot be. */
  const commit = (text: string, inUnit: Unit) => {
    const parsed = parse(text, inUnit)
    if ("problem" in parsed) {
      setProblem(parsed.problem)
      return
    }
    setProblem("")
    void submit(
      { mode: "after", seconds: parsed.seconds },
      { amount: parsed.seconds / units[inUnit].seconds, unit: inUnit },
    )
  }
  const lasting = selection !== "" && selection !== "default" && selection !== "never"
  // A choice saves as it is made: there is no draft to keep or discard. The
  // amount saves when it is entered (↩ or leaving the field).
  const options: ChoiceOption<Choosing>[] = [
    ...(channelId !== undefined ? [{ id: "default" as const, label: "Default" }] : []),
    { id: "never", label: "Never" },
    ...[...offered, ...(unit === "seconds" ? (["seconds"] as const) : [])].map(
      (each) => ({
        id: each,
        label: Number(amount) === 1 ? units[each].one : units[each].many,
        section: "Stop after",
      }),
    ),
  ]
  const status = settings
    ? problem
      ? undefined
      : selection === "default"
        ? `Default: ${label(settings.defaultLifetime)}`
        : selection === "never"
          ? "VMs keep billing until you stop them in Boat."
          : undefined
    : gateway
      ? error
        ? undefined
        : "Loading…"
      : "Connect a gateway"
  const ready = !!gateway && !!settings
  return (
    <div
      role="group"
      data-boat-runtime-settings={channelId ?? "global"}
      data-selection={selection || undefined}
      aria-label="Boat VM runtime settings"
      className="settings-boat-runtime"
    >
      <ItemRow
        label="Auto-stop VMs"
        detail={status}
        unavailable={!ready || busy}
        describedBy={problem ? problemId : undefined}
        control={
          // Keyed: the row maps a wrapper's children to name its controls.
          <span className="settings-boat-lifetime">
            {lasting ? (
              <Input
                key="amount"
                className="settings-boat-amount"
                type="text"
                inputMode="numeric"
                aria-label={`Auto-stop VMs after, in ${units[unit].many}`}
                aria-invalid={problem ? true : undefined}
                aria-describedby={problem ? problemId : undefined}
                data-boat-runtime-amount=""
                disabled={!ready}
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
                // Leaving the field enters it too, but not as it goes away
                // with its dialog: closing drops what was typed.
                onBlur={() => {
                  if (mounted.current) commit(amount, unit)
                }}
              />
            ) : null}
            <Choice
              key="unit"
              label="Auto-stop VMs"
              value={
                selection === "default" || selection === "never"
                  ? selection
                  : selection
                    ? unit
                    : undefined
              }
              options={options}
              onChange={(next) => {
                if (next === "default" || next === "never") {
                  setProblem("")
                  void submit({ mode: next })
                  return
                }
                // From Never or Default a length starts at one of the unit.
                const text = lasting ? amount : "1"
                setAmount(text)
                setUnit(next)
                commit(text, next)
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
      {error && (
        <p role="alert" className="settings-boat-note">
          {error}{" "}
          <Button
            className="settings-button"
            type="button"
            onClick={() => setRetry((value) => value + 1)}
          >
            Reload
          </Button>
        </p>
      )}
    </div>
  )
}
