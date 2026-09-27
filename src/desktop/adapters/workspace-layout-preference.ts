import { parseWorkspaceLayout } from "../model/workspace-layout"
import { storedPreference } from "./stored-preference"

/** The chosen workspace layout, remembered; the window and Settings follow it together. */
const workspaceLayoutPreference = storedPreference({
  key: "nessa.desktop.workspace-layout",
  event: "nessa:workspace-layout",
  parse: parseWorkspaceLayout,
})

export const useWorkspaceLayoutPreference = workspaceLayoutPreference.usePreference
