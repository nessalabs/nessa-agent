import { Button } from "@nessa-ui/react/button"
import { useEffect, useId, useRef, useState } from "react"
import type {
  BoatRuntimeSetParams,
  BoatRuntimeSettings,
  BoatVmLifetime,
} from "@nessa/client"
import type { BoatGateway } from "../adapters/boat-gateway"

const durations = [3600, 14_400, 28_800, 86_400] as const
function label(lifetime: BoatVmLifetime): string {
  if (lifetime.mode === "never") return "never"
  const seconds = lifetime.seconds
  if (seconds === undefined) return "unknown"
  return seconds % 3600 === 0
    ? `${seconds / 3600} ${seconds === 3600 ? "hour" : "hours"}`
    : `${seconds} seconds`
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
  const id = useId()
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
  return (
    <div
      role="group"
      data-boat-runtime-settings={channelId ?? "global"}
      aria-label="Boat VM runtime settings"
      className="settings-boat-runtime"
    >
      <label htmlFor={id}>Auto-stop VMs</label>
      <select
        id={id}
        className="settings-input settings-boat-select"
        value={selection}
        disabled={!gateway || !settings || busy}
        onChange={(event) => {
          setSelection(event.target.value)
          void submit(event.target.value)
        }}
      >
        {!settings && (
          <option value="">
            {gateway ? (error ? "Unavailable" : "Loading…") : "Connect a gateway"}
          </option>
        )}
        {channelId !== undefined && (
          <option value="default">
            Use Nessa default{settings ? ` (${label(settings.defaultLifetime)})` : ""}
          </option>
        )}
        <option value="never">Never</option>
        {durations.map((seconds) => (
          <option key={seconds} value={String(seconds)}>
            After {seconds / 3600} {seconds === 3600 ? "hour" : "hours"}
          </option>
        ))}
        {settings &&
          saved !== "default" &&
          saved !== "never" &&
          !durations.some((value) => String(value) === saved) && (
            <option value={saved}>
              After{" "}
              {label(
                channelId === undefined
                  ? settings.defaultLifetime
                  : (settings.channelOverride ?? settings.defaultLifetime),
              )}
            </option>
          )}
      </select>
      {selection === "never" && (
        <p className="settings-boat-note">
          VMs keep billing until you stop them in Boat.
        </p>
      )}
      {error && (
        <p role="alert">
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
