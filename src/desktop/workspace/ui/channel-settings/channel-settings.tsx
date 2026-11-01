import { useRef, useState } from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { IconButton } from "../../../ui/icon-button"
import { useBoatGateway } from "../../../settings/ui/boat-row"
import { BoatRuntimeControl } from "../../../settings/ui/boat-runtime-control"
import { gatewayChannelId } from "../../adapters/gateway/gateway-views"

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
            <BoatRuntimeControl channelId={scope} gateway={gateway} />
          </div>
        </Dialog>
      )}
    </>
  )
}
