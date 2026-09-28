import { useLayoutEffect } from "react"
import { parseFlag, parseOptIn } from "../model/window-preferences"
import { storedPreference } from "./stored-preference"

/**
 * The window's own on-or-off preferences, chosen in Settings and remembered
 * like its others (`stored-preference.ts`):
 *
 * - **greeting** — "Working late?" above a new session's composer;
 * - **drifting light** — the slow drift of the light behind the window,
 *   carried on the page's root as `data-drift` for the stylesheet;
 * - **⌘-click opens beside** — a row ⌘-clicked, or ⌘↩'d, opens beside the
 *   focused pane; off, it opens in its place;
 * - **running first** — the session list keeps running sessions in a group
 *   above the rest; off, they are listed with the rest, newest first;
 * - **picture in conversations** — a sliver of the header picture, or the
 *   night scene, at the top of each conversation pane (`HeaderSliver`);
 * - **agents overview** — a preview, off unless turned on (`parseOptIn`):
 *   whether the sidebar offers "Agents" and ⌘0 opens it. It decides only
 *   whether the way in is offered; the overview itself is workspace state
 *   (`content`), which an agent can show whatever this says.
 */
const flag = (key: string, event: string) =>
  storedPreference({
    key: `nessa.desktop.${key}`,
    event: `nessa:${event}`,
    parse: parseFlag,
  })

export const useGreetingPreference = flag("greeting", "desktop-greeting").usePreference
const driftPreference = flag("drifting-light", "desktop-drifting-light")
export const useDriftPreference = driftPreference.usePreference
export const useBesidePreference = flag(
  "cmd-click-beside",
  "desktop-cmd-click-beside",
).usePreference
export const useRunningFirstPreference = flag(
  "running-first",
  "desktop-running-first",
).usePreference

export const usePictureInConversationsPreference = flag(
  "picture-in-conversations",
  "desktop-picture-in-conversations",
).usePreference

export const useAgentsOverviewPreference = storedPreference({
  key: "nessa.desktop.experiments.agents-overview",
  event: "nessa:desktop-experiments-agents-overview",
  parse: parseOptIn,
}).usePreference

/** Keeps the root's `data-drift` to the drifting-light preference. Mounted once, by the window. */
export function useDriftInEffect(): void {
  const [drift] = useDriftPreference()
  useLayoutEffect(() => {
    document.documentElement.dataset.drift = drift === "on" ? "drifting" : "still"
  }, [drift])
}
