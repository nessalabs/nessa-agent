import { useRef, useState } from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { IconButton } from "../../../ui/icon-button"
import { useBoatConnection, useBoatGateway } from "../../../settings/ui/boat-row"
import { BoatRuntimeControl } from "../../../settings/ui/boat-runtime-control"
import { ItemRow, Segmented } from "../../../settings/ui/settings-controls"
import { channelFailure, type ChannelPlacement } from "../../application/ports"
import { gatewayChannelId } from "../../adapters/gateway/gateway-views"
import { PoolOrder, useDevicesGateway } from "../../../devices"
import { setChannelPlacement } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel } from "../../adapters/store/selectors"

type Place = "local" | "boat"
type ThreadPlace = Place | "pool"

/**
 * Where a made channel's agents run, changed after it was made. Each choice
 * saves as it is made. Threads already made keep running where they
 * started; moving the main agent starts a fresh main chat there, and the old
 * one stays in the channel as a thread.
 */
export function ChannelPlacementControl({ channelId }: { channelId: string }) {
  const dispatch = useWorkspaceDispatch()
  const channel = useWorkspaceSelector((state) => selectChannel(state, channelId))
  const [saving, setSaving] = useState(false)
  const [failed, setFailed] = useState<string | null>(null)
  // As when a channel is made, Boat is offered only once it is connected:
  // agents placed there before that could not start.
  const boatReady = useBoatConnection().status?.state === "connected"
  if (!channel) return null
  const places: readonly { id: Place; label: string; disabled?: boolean }[] = [
    { id: "local", label: "This Mac" },
    {
      id: "boat",
      label: "Boat",
      disabled: !boatReady,
    },
  ]
  // Only threads spread over computers: the main agent stays put.
  const threadPlaces: readonly { id: ThreadPlace; label: string; disabled?: boolean }[] =
    [...places, { id: "pool", label: "Any" }]
  const placement: ChannelPlacement = {
    coordinatorPlacement: channel.coordinatorPlacement ?? "local",
    threadPlacement: channel.threadPlacement ?? "local",
  }
  const change = (next: Partial<ChannelPlacement>) => {
    if (saving) return
    const asked = { ...placement, ...next }
    if (
      asked.coordinatorPlacement === placement.coordinatorPlacement &&
      asked.threadPlacement === placement.threadPlacement
    )
      return
    setSaving(true)
    setFailed(null)
    void dispatch(setChannelPlacement({ channelId, placement: asked }))
      .then((changed) => {
        if (!changed) setFailed("This window cannot change where channels run.")
      })
      .catch((error: unknown) => {
        setFailed(
          channelFailure(error) === "forbidden"
            ? "You can’t change where this channel runs."
            : "Couldn’t save that. Try again in a moment.",
        )
      })
      .finally(() => setSaving(false))
  }
  return (
    <div
      role="group"
      aria-label="Where this channel runs"
      aria-busy={saving || undefined}
      data-channel-placement={channelId}
      data-coordinator-placement={placement.coordinatorPlacement}
      data-thread-placement={placement.threadPlacement}
      className="settings-channel-placement"
    >
      {/* Each setting stacks its name over its control, so neither is squeezed
          in a narrow dialog. */}
      <ItemRow
        label="Main agent runs on"
        detail={boatReady ? undefined : "Connect Boat to run agents there."}
        data-placement-row="coordinator"
        unavailable={saving}
      >
        <Segmented<Place>
          label="Main agent runs on"
          value={placement.coordinatorPlacement}
          options={places}
          onChange={(next) => change({ coordinatorPlacement: next })}
        />
      </ItemRow>
      <ItemRow
        label="New threads run on"
        data-placement-row="threads"
        unavailable={saving}
      >
        <Segmented<ThreadPlace>
          label="New threads run on"
          value={placement.threadPlacement}
          options={threadPlaces}
          onChange={(next) => change({ threadPlacement: next })}
        />
      </ItemRow>
      {failed ? (
        <p role="alert" className="settings-boat-note">
          {failed}
        </p>
      ) : null}
    </div>
  )
}

/** The same settings editor opens from either channel-list layout. */
export function ChannelSettingsButton({
  channelId,
  name,
}: {
  channelId: string
  name: string
}) {
  const [open, setOpen] = useState(false)
  const trigger = useRef<HTMLButtonElement>(null)
  const gateway = useBoatGateway()
  const devices = useDevicesGateway()
  const scope = gatewayChannelId(channelId)
  return (
    <>
      <IconButton
        ref={trigger}
        data-channel-settings-open={channelId}
        icon="preferences"
        label={`Settings for #${name}`}
        size="sm"
        onClick={(event) => {
          event.stopPropagation()
          setOpen(true)
        }}
      />
      {open && (
        <Dialog
          open
          data-channel-settings=""
          onOpenChange={setOpen}
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            trigger.current?.focus()
          }}
          size="sm"
          title={`#${name} settings`}
          description={scope === undefined ? "The default for every channel." : undefined}
        >
          <div className="settings-boat settings-surface">
            {scope === undefined ? null : (
              <ChannelPlacementControl channelId={channelId} />
            )}
            <BoatRuntimeControl channelId={scope} gateway={gateway} />
          </div>
          {/* The default channel follows Devices' order, set there. */}
          {scope !== undefined && devices ? (
            <div className="settings-surface" data-channel-settings-pool="">
              <PoolOrder gateway={devices} channelId={scope} />
            </div>
          ) : null}
        </Dialog>
      )}
    </>
  )
}
