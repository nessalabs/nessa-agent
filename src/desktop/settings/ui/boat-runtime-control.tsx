import { Button } from "@nessa-ui/react/button"
import { useEffect, useRef, useState } from "react"
import type {
  BoatRuntimeSetParams,
  BoatRuntimeSettings,
  BoatVmLifetime,
} from "@nessa/client"
import type { BoatGateway } from "../adapters/boat-gateway"
import { ItemRow, Segmented } from "./settings-controls"

const durations = [3600, 14_400, 28_800, 86_400] as const
function label(lifetime: BoatVmLifetime): string {
  if (lifetime.mode === "never") return "never"
  const seconds = lifetime.seconds
  if (seconds === undefined) return "unknown"
  return seconds % 3600 === 0
    ? `${seconds / 3600} ${seconds === 3600 ? "hour" : "hours"}`
    : `${seconds} seconds`
}
/** The same length, as a choice's short label: "2h", "90s". */
function short(lifetime: BoatVmLifetime): string {
  if (lifetime.mode === "never") return "Never"
  const seconds = lifetime.seconds ?? 0
  return seconds % 3600 === 0 ? `${seconds / 3600}h` : `${seconds}s`
}
function key(lifetime: BoatVmLifetime | undefined): string {
  return lifetime === undefined
    ? "default"
    : lifetime.mode === "never"
      ? "never"
      : String(lifetime.seconds)
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
  const [settings, setSettings] = useState<BoatRuntimeSettings>()
  const [selection, setSelection] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [retry, setRetry] = useState(0)
  useEffect(() => {
    const current = ++revision.current
    setSettings(undefined)
    setSelection("")
    setError("")
    setBusy(false)
    savePending.current = false
    void gateway
      ?.runtimeSettings(channelId === undefined ? {} : { channelId })
      .then((value) => {
        if (current !== revision.current) return
        setSettings(value)
        setSelection(
          key(channelId === undefined ? value.defaultLifetime : value.channelOverride),
        )
      })
      .catch(() => {
        if (current === revision.current) setError("Couldn’t read this.")
      })
    return () => {
      revision.current += 1
    }
  }, [gateway, channelId, retry])
  const saved =
    settings &&
    key(channelId === undefined ? settings.defaultLifetime : settings.channelOverride)
  const submit = async (selection: string) => {
    if (!gateway || !settings || busy || savePending.current || selection === saved)
      return
    let choice: Pick<BoatRuntimeSetParams, "mode" | "seconds">
    if (selection === "default") choice = { mode: "default" }
    else if (selection === "never") choice = { mode: "never" }
    else {
      const seconds = durations.find((value) => String(value) === selection)
      if (seconds === undefined) return
      choice = { mode: "after", seconds }
    }
    const current = ++revision.current
    savePending.current = true
    setBusy(true)
    setError("")
    try {
      const value = await gateway.setRuntimeSettings({
        ...choice,
        ...(channelId === undefined ? {} : { channelId }),
      })
      if (current !== revision.current) return
      setSettings(value)
      setSelection(
        key(channelId === undefined ? value.defaultLifetime : value.channelOverride),
      )
    } catch {
      if (current === revision.current) {
        setError("Couldn’t confirm the change.")
        setSettings(undefined)
      }
    } finally {
      if (current === revision.current) {
        savePending.current = false
        setBusy(false)
      }
    }
  }
  // A choice saves as it is made: there is no draft to keep or discard.
  const options: { id: string; label: string }[] = [
    ...(channelId !== undefined ? [{ id: "default", label: "Default" }] : []),
    { id: "never", label: "Never" },
    ...durations.map((seconds) => ({ id: String(seconds), label: `${seconds / 3600}h` })),
  ]
  // A saved length the choices do not offer (set elsewhere) is offered too.
  if (settings && saved !== undefined && !options.some((option) => option.id === saved))
    options.push({
      id: saved,
      label: short(
        channelId === undefined
          ? settings.defaultLifetime
          : (settings.channelOverride ?? settings.defaultLifetime),
      ),
    })
  const status = settings
    ? selection === "default"
      ? `Default: ${label(settings.defaultLifetime)}`
      : selection === "never"
        ? "VMs keep billing until you stop them in Boat."
        : undefined
    : gateway
      ? error
        ? undefined
        : "Loading…"
      : "Connect a gateway"
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
        unavailable={!gateway || !settings || busy}
      >
        <Segmented
          label="Auto-stop VMs"
          value={selection}
          options={options}
          onChange={(next) => {
            setSelection(next)
            void submit(next)
          }}
        />
      </ItemRow>
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
