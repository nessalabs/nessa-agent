import { useRef, useState } from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { IconButton } from "../../../ui/icon-button"
import {
  ReplyStyleControl,
  useReplyStyleGateway,
} from "../../../settings/ui/reply-style-control"
import { Choice, ItemRow } from "../../../settings/ui/settings-controls"
import { channelFailure } from "../../application/ports"
import { gatewayChannelId } from "../../adapters/gateway/gateway-views"
import { setChannelCoordinator } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel } from "../../adapters/store/selectors"

/**
 * Whether a channel has a coordinator, switched at any time: a made one, or
 * the default one, Conversations. Off keeps
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
  const replyStyle = useReplyStyleGateway()
  const scope = gatewayChannelId(channelId)
  const switchable = useWorkspaceSelector(
    (state) => selectChannel(state, channelId)?.coordinatorOn !== undefined,
  )
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
          {/* Every agent runs on this gateway's own machine (ADR 1417), so a
          channel has no place to choose: only whether it has a coordinator.
          The default channel has one too. */}
          {switchable ? (
            <div className="settings-boat settings-surface channel-settings-section">
              <ChannelCoordinatorControl channelId={channelId} />
            </div>
          ) : null}
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
        </Dialog>
      )}
    </>
  )
}
