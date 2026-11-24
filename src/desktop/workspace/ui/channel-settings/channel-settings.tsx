import { useRef, useState } from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { IconButton } from "../../../ui/icon-button"
import { useBoatGateway } from "../../../settings/ui/boat-row"
import { BoatRuntimeControl } from "../../../settings/ui/boat-runtime-control"
import {
  ReplyStyleControl,
  useReplyStyleGateway,
} from "../../../settings/ui/reply-style-control"
import { Choice, ItemRow } from "../../../settings/ui/settings-controls"
import type { ChoiceOption } from "../../../ui/choice-menu"
import { channelFailure, type ChannelPlacement } from "../../application/ports"
import { gatewayChannelId } from "../../adapters/gateway/gateway-views"
import { PoolOrder, useDevicesGateway } from "../../../devices"
import { channelPlaceKind, type ChannelPlace } from "../../model/channel-place"
import { cloudChoices, type CloudChoice } from "../../../clouds/model/clouds"
import { useCloudStates } from "../../../clouds/ui/cloud-states"
import { channelPlaceWords } from "../channel-place-words"
import { setChannelCoordinator, setChannelPlacement } from "../../adapters/store/commands"
import { useComputers, type Computer } from "./computers"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel } from "../../adapters/store/selectors"

/**
 * One of the two settings: where its agents run, chosen from a menu at the
 * row's end. This Mac, Boat and Any come first; then each computer connected
 * now, by name. A computer saved earlier and not connected now is still
 * listed, so the menu says where the channel runs.
 */
function PlaceRow({
  row,
  label,
  value,
  computers,
  clouds,
  saving,
  onChange,
}: {
  row: "coordinator" | "threads"
  label: string
  value: ChannelPlace
  computers: readonly Computer[]
  clouds: readonly CloudChoice[]
  saving: boolean
  onChange: (next: ChannelPlace) => void
}) {
  const options: ChoiceOption<ChannelPlace>[] = [
    { id: "local", label: "This Mac" },
    ...clouds.map((cloud) => ({
      id: cloud.id,
      label: cloud.label,
      disabled: cloud.disabled,
    })),
    { id: "pool", label: "Any computer" },
    ...computers.map((computer) => ({
      id: computer.environment,
      label: computer.name,
      section: "Computers",
    })),
  ]
  if (
    channelPlaceKind(value) === "computer" &&
    !options.some((each) => each.id === value)
  )
    options.push({
      id: value,
      label: channelPlaceWords(value, undefined),
      section: "Computers",
    })
  return (
    <ItemRow
      label={label}
      detail={
        row === "coordinator" && clouds.every((cloud) => cloud.disabled)
          ? "Connect a cloud in Settings › Accounts to run agents there."
          : undefined
      }
      data-placement-row={row}
      unavailable={saving}
      control={
        <Choice label={label} value={value} options={options} onChange={onChange} />
      }
    />
  )
}

/**
 * Whether a made channel has a coordinator, switched at any time. Off keeps
 * its chat and its threads; the channel just opens without it. On again,
 * the same chat is its coordinator. Saves as it is chosen.
 */
export function ChannelCoordinatorControl({ channelId }: { channelId: string }) {
  const dispatch = useWorkspaceDispatch()
  const channel = useWorkspaceSelector((state) => selectChannel(state, channelId))
  const [saving, setSaving] = useState(false)
  const [failed, setFailed] = useState<string | null>(null)
  if (!channel || channel.coordinatorOn === undefined) return null
  const on = channel.coordinatorOn
  const change = (next: "on" | "off") => {
    if (saving || (next === "on") === on) return
    setSaving(true)
    setFailed(null)
    void dispatch(setChannelCoordinator({ channelId, on: next === "on" }))
      .then((changed) => {
        if (!changed) setFailed("This window cannot change this channel.")
      })
      .catch((error: unknown) => {
        setFailed(
          channelFailure(error) === "forbidden"
            ? "You can’t change this channel."
            : "Couldn’t save that. Try again in a moment.",
        )
      })
      .finally(() => setSaving(false))
  }
  return (
    <div
      role="group"
      aria-label="Coordinator"
      aria-busy={saving || undefined}
      data-channel-coordinator={channelId}
      data-coordinator-on={on ? "true" : "false"}
    >
      <ItemRow
        label="Coordinator"
        detail={
          on
            ? "Starts threads and keeps you up to date. The channel opens at its chat."
            : "Off. Its chat and threads stay. Turn it on to pick up the same chat."
        }
        unavailable={saving}
        control={
          <Choice
            label="Coordinator"
            value={on ? "on" : "off"}
            options={[
              { id: "on", label: "On" },
              { id: "off", label: "Off" },
            ]}
            onChange={change}
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
  // As when a channel is made, a cloud is offered only once it is
  // connected: agents placed there before that could not start.
  const states = useCloudStates()
  const clouds = cloudChoices((id) => states[id] === "connected")
  const computers = useComputers()
  if (!channel) return null
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
        const failure = channelFailure(error)
        setFailed(
          failure === "forbidden"
            ? "You can’t change where this channel runs."
            : failure === "place-unavailable"
              ? "That computer isn’t connected now. Choose another place."
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
      <PlaceRow
        row="coordinator"
        label="Main agent runs on"
        value={placement.coordinatorPlacement}
        computers={computers}
        clouds={clouds}
        saving={saving}
        onChange={(next) => change({ coordinatorPlacement: next })}
      />
      <PlaceRow
        row="threads"
        label="New threads run on"
        value={placement.threadPlacement}
        computers={computers}
        clouds={clouds}
        saving={saving}
        onChange={(next) => change({ threadPlacement: next })}
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
  // The dialog opens inside the window's surface, as Settings' do, so its rows
  // read the desktop's type and tones; portalled to the body they have none
  // and fall back to the page's larger text.
  const [home, setHome] = useState<HTMLElement | null>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const gateway = useBoatGateway()
  const devices = useDevicesGateway()
  const replyStyle = useReplyStyleGateway()
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
          setHome(event.currentTarget.closest<HTMLElement>("[data-surface]"))
          setOpen(true)
        }}
      />
      {open && (
        <Dialog
          open
          container={home}
          data-channel-settings=""
          onOpenChange={setOpen}
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            trigger.current?.focus()
          }}
          size="md"
          title={`#${name} settings`}
          description={scope === undefined ? "The default for every channel." : undefined}
        >
          <div className="settings-boat settings-surface channel-settings-section">
            {scope === undefined ? null : (
              <>
                <ChannelCoordinatorControl channelId={channelId} />
                <ChannelPlacementControl channelId={channelId} />
              </>
            )}
            <BoatRuntimeControl channelId={scope} gateway={gateway} />
          </div>
          {/* The default channel's dialog sets the Nessa level, which every
          channel follows unless it has its own. */}
          {replyStyle ? (
            <div
              className="settings-surface channel-settings-section"
              data-channel-settings-reply-style=""
            >
              <h3 className="channel-settings-heading">How agents talk</h3>
              <ReplyStyleControl channelId={scope} gateway={replyStyle} />
            </div>
          ) : null}
          {/* The default channel follows Devices' order, set there. */}
          {scope !== undefined && devices ? (
            <div
              className="settings-surface channel-settings-section"
              data-channel-settings-pool=""
            >
              <h3 className="channel-settings-heading">Order for “Any”</h3>
              <PoolOrder gateway={devices} channelId={scope} />
            </div>
          ) : null}
        </Dialog>
      )}
    </>
  )
}
