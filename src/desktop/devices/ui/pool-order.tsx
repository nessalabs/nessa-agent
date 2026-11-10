/**
 * Where new threads on "Any computer" run: the paired computers in order,
 * first choice first, each let in or left out. Devices edits the Nessa
 * default; a channel's settings either follow it or keep an order of their
 * own. Every change saves at once (`devices.setPool`); what is shown after
 * is what the gateway answered.
 */
import { Button } from "@nessa-ui/react/button"
import {
  SegmentedControl,
  SegmentedControlOption,
} from "@nessa-ui/react/segmented-control"
import { SettingsGroup, SettingsRow } from "@nessa-ui/react/settings-group"
import { Switch } from "@nessa-ui/react/switch"
import { useEffect, useRef, useState } from "react"
import type { DevicesPool } from "@nessa/client"
import { IconButton } from "../../ui/icon-button"
import type { DevicesGateway } from "../adapters/devices-gateway"
import { sortWorkers, statusWord, workerCard, type WorkerCard } from "../model/devices"
import {
  includeRow,
  moveRow,
  poolRows,
  preferenceOf,
  samePreference,
  type PoolPreference,
  type PoolRow,
} from "../model/pool"

const chevron = (up: boolean) => (
  <svg
    viewBox="0 0 16 16"
    width="14"
    height="14"
    aria-hidden="true"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.8"
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    <path d={up ? "M4 10l4-4 4 4" : "M4 6l4 4 4-4"} />
  </svg>
)

type Mode = "default" | "custom"

/**
 * The order editor. `channelId` (a gateway channel id) edits that channel's
 * own order, with a choice to follow the Nessa default; absent edits the
 * default. `workers` are the computers to order when the host has read
 * them; absent, the editor reads them itself.
 */
export function PoolOrder({
  gateway,
  channelId,
  workers,
}: {
  gateway: Pick<DevicesGateway, "list" | "pool" | "setPool"> | undefined
  channelId?: string
  workers?: readonly WorkerCard[]
}) {
  const revision = useRef(0)
  const [pool, setPool] = useState<DevicesPool>()
  const [listed, setListed] = useState<readonly WorkerCard[]>()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [retry, setRetry] = useState(0)
  // The host's list is read on its own poll; the editor reads one only
  // when it was given none, and once, not each time the host's changes.
  const readsList = workers === undefined
  useEffect(() => {
    const current = ++revision.current
    setPool(undefined)
    setError("")
    setBusy(false)
    if (!gateway) return
    void Promise.all([
      gateway.pool(channelId),
      readsList ? gateway.list() : Promise.resolve(undefined),
    ]).then(([read, list]) => {
      if (current !== revision.current) return
      if (!read.ok) {
        setError("Couldn’t read the order.")
        return
      }
      setPool(read.value)
      if (list?.ok) setListed(sortWorkers(list.value.workers.map(workerCard)))
    })
    return () => {
      revision.current += 1
    }
  }, [gateway, channelId, retry, readsList])
  const computers = workers ?? listed ?? []
  const mode: Mode =
    channelId === undefined || pool?.channelOverride !== undefined ? "custom" : "default"
  const preference: PoolPreference | undefined = pool
    ? (pool.channelOverride ?? pool.defaultPreference)
    : undefined
  const rows = preference ? poolRows(computers, preference) : []
  const save = async (next: PoolPreference | undefined) => {
    if (!gateway || !pool || busy) return
    if (next && preference && mode === "custom" && samePreference(next, preference))
      return
    const current = ++revision.current
    setBusy(true)
    setError("")
    const saved = await gateway.setPool(next, channelId)
    if (current !== revision.current) return
    setBusy(false)
    if (saved.ok) setPool(saved.value)
    else {
      setError("Couldn’t confirm the change.")
      setPool(undefined)
    }
  }
  const change = (next: PoolRow[]) => {
    if (preference) void save(preferenceOf(next, preference))
  }
  return (
    <div className="devices-pool" data-devices-pool={channelId ?? "global"}>
      {channelId !== undefined ? (
        <SegmentedControl
          variant="glass"
          className="devices-pool-mode"
          aria-label="Where this channel’s threads run"
          value={mode}
          onValueChange={(next) => {
            if (!pool || next === mode) return
            // Keeping an order of its own starts from the default's.
            void save(next === "custom" ? pool.defaultPreference : undefined)
          }}
        >
          <SegmentedControlOption
            value="default"
            disabled={!pool || busy}
            data-devices-pool-mode="default"
          >
            Same as Devices
          </SegmentedControlOption>
          <SegmentedControlOption
            value="custom"
            disabled={!pool || busy}
            data-devices-pool-mode="custom"
          >
            Its own order
          </SegmentedControlOption>
        </SegmentedControl>
      ) : null}
      <SettingsGroup
        surface="flat"
        aria-label="Thread order"
        disabled={!pool || busy || mode === "default"}
        footnote={
          pool
            ? `New threads on Any computer go to the first one here that’s online and under ${pool.busyCpuPercent}% CPU and ${pool.busyMemoryPercent}% memory. When every one is busy, Boat takes it if it’s connected, or else the first one here.`
            : undefined
        }
      >
        {!pool ? (
          <SettingsRow
            label={gateway ? (error ? "Unavailable" : "Loading…") : "Connect a gateway"}
          />
        ) : rows.length === 0 ? (
          <SettingsRow
            label="No computers yet"
            detail="Add a computer and it joins the order."
            data-devices-pool-empty=""
          />
        ) : (
          rows.map((row, index) => (
            <SettingsRow
              key={row.environment}
              data-devices-pool-row={row.environment}
              data-included={row.included ? "true" : "false"}
              leading={
                <span className="devices-pool-place" aria-hidden="true">
                  {row.included
                    ? rows.slice(0, index + 1).filter((each) => each.included).length
                    : "–"}
                </span>
              }
              label={row.name}
              detail={row.included ? statusWord(row.status) : "Left out"}
              control={
                <span className="devices-pool-controls">
                  <IconButton
                    key="up"
                    icon={chevron(true)}
                    label={`Move ${row.name} up`}
                    size="sm"
                    disabled={index === 0 || mode === "default" || busy}
                    onClick={() => change(moveRow(rows, index, -1))}
                    data-devices-pool-up={row.environment}
                  />
                  <IconButton
                    key="down"
                    icon={chevron(false)}
                    label={`Move ${row.name} down`}
                    size="sm"
                    disabled={index === rows.length - 1 || mode === "default" || busy}
                    onClick={() => change(moveRow(rows, index, 1))}
                    data-devices-pool-down={row.environment}
                  />
                  <Switch
                    key="include"
                    aria-label={`Send threads to ${row.name}`}
                    checked={row.included}
                    onCheckedChange={(included) =>
                      change(includeRow(rows, index, included))
                    }
                    data-devices-pool-include={row.environment}
                  />
                </span>
              }
            />
          ))
        )}
      </SettingsGroup>
      {error ? (
        <p role="alert" className="devices-muted">
          {error}{" "}
          <Button
            className="settings-button"
            type="button"
            onClick={() => setRetry((value) => value + 1)}
          >
            Reload
          </Button>
        </p>
      ) : null}
    </div>
  )
}
