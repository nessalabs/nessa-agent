/**
 * Settings › Accounts › Default cloud: where a thread's VM is made when its
 * channel names no cloud (a chat in the default channel, a pool's overflow).
 * Shown only while two clouds are connected: with one, that one is where
 * every such VM goes, and there is nothing to choose.
 */
import { useContext, useEffect, useState } from "react"
import { setting } from "../../settings/model/settings-catalogue"
import { useAgent37Gateway } from "../../settings/ui/agent37-row"
import { Choice, FoundSetting, ItemRow } from "../../settings/ui/settings-controls"
import { cloudChoices, type Cloud } from "../model/clouds"
import { useCloudStates } from "./cloud-states"

export function DefaultCloudRow({ seen }: { seen?: unknown }) {
  // The gateway's cloud choice is read and changed through the same
  // client the Agent37 row uses.
  const gateway = useAgent37Gateway()
  const entry = setting("cloud-choice")
  const found = useContext(FoundSetting) === "cloud-choice"
  const states = useCloudStates(seen)
  const [cloud, setCloud] = useState<Cloud>()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const connected = (id: Cloud) => states[id] === "connected"
  const offered = cloudChoices(connected)
  const choosing = offered.filter((each) => !each.disabled).length >= 2
  useEffect(() => {
    if (!gateway || !choosing) return
    let live = true
    gateway.cloud().then(
      (choice) => live && setCloud(choice.cloud),
      () => live && setError("The default cloud could not be read."),
    )
    return () => {
      live = false
    }
  }, [gateway, choosing, seen])
  if (!gateway || !choosing) return null
  return (
    <ItemRow
      setting="cloud-choice"
      found={found}
      label={entry.label}
      detail={
        error ||
        "For a thread whose channel names no cloud. A channel picks its own in its settings; a thread whose VM exists stays on the cloud it was made on."
      }
      unavailable={cloud === undefined || busy}
      data-cloud-choice={cloud ?? "reading"}
      control={
        <Choice
          label={entry.label}
          value={cloud}
          options={offered.map(({ id, label, disabled }) => ({ id, label, disabled }))}
          onChange={(next: Cloud) => {
            setBusy(true)
            setError("")
            gateway
              .chooseCloud(next)
              .then(
                (choice) => setCloud(choice.cloud),
                () => setError("The default cloud could not be changed. Try again."),
              )
              .finally(() => setBusy(false))
          }}
        />
      }
    />
  )
}
