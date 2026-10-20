import type { SwitcherRow } from "../../model/session-search"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectFocusedChannel } from "../../adapters/store/selectors"
import { QuickSwitcher, type SwitcherMode } from "../quick-switcher/quick-switcher"

/** The switcher, starting new sessions in the focused pane's channel. */
export function SwitcherHost({
  mode,
  onClose,
  onPick,
}: {
  mode: SwitcherMode
  onClose: () => void
  onPick: (row: SwitcherRow, beside: boolean) => void
}) {
  const channelId = useWorkspaceSelector(selectFocusedChannel) ?? ""
  return (
    <QuickSwitcher mode={mode} channelId={channelId} onClose={onClose} onPick={onPick} />
  )
}
