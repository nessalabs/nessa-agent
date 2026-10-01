import { memo } from "react"
import { useNativePlugins } from "../../../widgets"
import { useAccessoryOpen } from "../../adapters/store/widget-hosts"

/**
 * What each widget plugin draws in a session pane's header for that session
 * (`SessionAccessory`, ADR 326), given its id and `openWidget` alone: what it
 * opens goes beside this session's pane. The workspace draws them without
 * importing any plugin; a plugin draws nothing for a session it has nothing
 * of.
 */
export const SessionAccessories = memo(function SessionAccessories({
  sessionId,
}: {
  sessionId: string
}) {
  const plugins = useNativePlugins()
  const openWidget = useAccessoryOpen(sessionId)
  if (!plugins.some((plugin) => plugin.SessionAccessory)) return null
  return (
    <div className="workspace-pane-accessories">
      {plugins.map(({ id, SessionAccessory }) =>
        SessionAccessory ? (
          <SessionAccessory key={id} sessionId={sessionId} openWidget={openWidget} />
        ) : null,
      )}
    </div>
  )
})
