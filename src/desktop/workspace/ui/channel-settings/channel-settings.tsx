import { useRef, useState } from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { IconButton } from "../../../ui/icon-button"
import { useBoatConnection, useBoatGateway } from "../../../settings/ui/boat-row"
import { BoatRuntimeControl } from "../../../settings/ui/boat-runtime-control"
import { ItemRow, Segmented } from "../../../settings/ui/settings-controls"
import { channelFailure, type ChannelPlacement } from "../../application/ports"
import { gatewayChannelId } from "../../adapters/gateway/gateway-views"
import { setChannelPlacement } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel } from "../../adapters/store/selectors"

type Place = "local" | "boat"

/**
 * Where a made channel's agents run, changed after it was made. Each choice
 * saves as it is made. Threads already made keep running where they
 * started; moving the main agent starts a fresh main chat there, and the old
 * one stays in the channel as a thread, so the row says so.
 */
function ChannelPlacementControl({ channelId }: { channelId: string }) {
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
      label: boatReady ? "Boat" : "Boat (not connected)",
      disabled: !boatReady,
    },
  ]
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
      <ItemRow
        label="Main agent runs on"
        detail="Moving it starts a fresh main chat there. The current one stays here as a thread."
        data-placement-row="coordinator"
        unavailable={saving}
        control={
          <Segmented<Place>
            label="Main agent runs on"
            value={placement.coordinatorPlacement}
            options={places}
            onChange={(next) => change({ coordinatorPlacement: next })}
          />
        }
      />
      <ItemRow
        label="New threads run on"
        detail="Threads already started keep running where they are."
        data-placement-row="threads"
        unavailable={saving}
        control={
          <Segmented<Place>
            label="New threads run on"
            value={placement.threadPlacement}
            options={places}
            onChange={(next) => change({ threadPlacement: next })}
          />
        }
      />
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
        </Dialog>
      )}
    </>
  )
}
