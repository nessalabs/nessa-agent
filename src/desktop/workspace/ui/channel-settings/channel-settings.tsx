import { useRef, useState } from "react"
import { Button } from "@nessa-ui/react/button"
import { Dialog } from "@nessa-ui/react/dialog"
import { IconButton } from "../../../ui/icon-button"
import { useBoatConnection, useBoatGateway } from "../../../settings/ui/boat-row"
import { BoatRuntimeControl } from "../../../settings/ui/boat-runtime-control"
import { ItemRow, Segmented } from "../../../settings/ui/settings-controls"
import { channelFailure, type ChannelPlacement } from "../../application/ports"
import { gatewayChannelId } from "../../adapters/gateway/gateway-views"
import { PoolOrder, useDevicesGateway } from "../../../devices"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuRadioGroup,
  MenuRadioItem,
} from "../../../ui/menu"
import {
  channelPlaceKind,
  type ChannelPlace,
  type ChannelPlaceKind,
} from "../../model/channel-place"
import { channelPlaceWords } from "../channel-place-words"
import { setChannelPlacement } from "../../adapters/store/commands"
import { useComputers, type Computer } from "./computers"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel } from "../../adapters/store/selectors"

/**
 * One of the two settings: a kind of place side by side, and, once
 * "Computer" is chosen, which computer, from a menu of the ones connected
 * now. Choosing "Computer" alone saves nothing: the place is saved when a
 * computer is chosen.
 */
function PlaceRow({
  row,
  label,
  value,
  computers,
  boatReady,
  saving,
  onChange,
}: {
  row: "coordinator" | "threads"
  label: string
  value: ChannelPlace
  computers: readonly Computer[]
  boatReady: boolean
  saving: boolean
  onChange: (next: ChannelPlace) => void
}) {
  const [picking, setPicking] = useState(false)
  const kind = channelPlaceKind(value)
  const shown: ChannelPlaceKind = picking ? "computer" : kind
  const listed = computers.find((each) => each.environment === value)
  const kinds: readonly { id: ChannelPlaceKind; label: string; disabled?: boolean }[] = [
    { id: "local", label: "This Mac" },
    { id: "boat", label: "Boat", disabled: !boatReady },
    { id: "pool", label: "Any" },
    {
      id: "computer",
      label: "Computer",
      disabled: computers.length === 0 && kind !== "computer",
    },
  ]
  return (
    <ItemRow
      label={label}
      detail={
        row === "coordinator" && !boatReady
          ? "Connect Boat to run agents there."
          : undefined
      }
      data-placement-row={row}
      unavailable={saving}
    >
      <Segmented<ChannelPlaceKind>
        label={label}
        value={shown}
        options={kinds}
        onChange={(next) => {
          if (next === "computer") {
            setPicking(kind !== "computer")
            return
          }
          setPicking(false)
          onChange(next)
        }}
      />
      {shown === "computer" ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="secondary"
              size="sm"
              disabled={saving}
              data-placement-computer={row}
              aria-label={`${label}: which computer`}
            >
              {kind === "computer"
                ? channelPlaceWords(value, listed?.name)
                : "Choose a computer"}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" sideOffset={6}>
            <MenuRadioGroup
              value={kind === "computer" ? value : ""}
              onValueChange={(next) => {
                setPicking(false)
                onChange(next)
              }}
            >
              {computers.map((computer) => (
                <MenuRadioItem
                  key={computer.environment}
                  value={computer.environment}
                  data-placement-choice={computer.environment}
                >
                  {computer.name}
                </MenuRadioItem>
              ))}
            </MenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
    </ItemRow>
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
  // As when a channel is made, Boat is offered only once it is connected:
  // agents placed there before that could not start.
  const boatReady = useBoatConnection().status?.state === "connected"
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
      {/* Each setting stacks its name over its control, so neither is squeezed
          in a narrow dialog. */}
      <PlaceRow
        row="coordinator"
        label="Main agent runs on"
        value={placement.coordinatorPlacement}
        computers={computers}
        boatReady={boatReady}
        saving={saving}
        onChange={(next) => change({ coordinatorPlacement: next })}
      />
      <PlaceRow
        row="threads"
        label="New threads run on"
        value={placement.threadPlacement}
        computers={computers}
        boatReady={boatReady}
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
              <ChannelPlacementControl channelId={channelId} />
            )}
            <BoatRuntimeControl channelId={scope} gateway={gateway} />
          </div>
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
