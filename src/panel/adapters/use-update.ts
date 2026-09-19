import * as React from "react"

import {
  availableUpdate,
  installUpdate,
  onUpdateAvailable,
  onUpdateFailed,
  onUpdateProgress,
} from "../../host"
import {
  afterUpdate,
  noUpdate,
  updateNotice,
  updateTab,
  type UpdateNotice,
  type UpdateTab,
} from "../application/update-surface"
import { attachThenAsk } from "../application/update-subscription"

/** What the panel needs from an update: two views and three gestures. */
export interface PanelUpdate {
  /** The notice above the composer, or `null`. */
  notice: UpdateNotice | null
  /** The update tab, or `null` when no install has been asked for. */
  tab: UpdateTab | null
  /** Whether the update tab is the one being looked at. */
  viewing: boolean
  /** Take the install: the notice goes and the tab opens on the download. */
  install: () => void
  /** Turn this version down for this launch. */
  dismiss: () => void
  /** Close a failed update tab. */
  close: () => void
  /** Look at the update tab, or away from it. */
  setViewing: (viewing: boolean) => void
}

/**
 * The panel's side of the update flow.
 *
 * Every decision is in `update-surface.ts` and tested there; what is left here
 * is the wiring a test cannot drive — three host subscriptions, one question
 * asked on mount, and the invoke that starts the download.
 *
 * The question on mount is not a second path to the same news. A check runs
 * once per launch and can finish before this page has listeners, or while the
 * panel is closed entirely; the host keeps what it announced and answers with
 * it, so both the event and the answer carry the one value the host holds.
 */
export function useUpdate(): PanelUpdate {
  const [state, report] = React.useReducer(afterUpdate, noUpdate)
  // Whether the update tab is selected, which is a view's business and not the
  // update's: closing the tab and switching to a conversation are the same
  // gesture as far as the download is concerned.
  const [selected, setSelected] = React.useState(false)

  React.useEffect(
    () =>
      attachThenAsk({
        // Every listener, and not resolved until each is really attached.
        attach: (live) =>
          Promise.all([
            onUpdateAvailable((release) => {
              if (live()) report({ kind: "announced", release })
            }),
            onUpdateProgress(({ downloaded, total }) => {
              if (live()) report({ kind: "progress", downloaded, total })
            }),
            onUpdateFailed((reason) => {
              if (!live()) return
              // The sentence on screen is the panel's; the host's own reason is
              // technical and belongs with the diagnostics, where a bug report
              // can find it. It is already on the host's stderr too.
              console.warn("[nessa] the update was not installed", reason)
              report({ kind: "failed" })
            }),
          ]),
        // The host holds what it announced, so this finds an update from before
        // the listeners existed; anything later reaches one of them.
        ask: async (live) => {
          const release = await availableUpdate()
          if (live() && release) report({ kind: "announced", release })
        },
        // Nothing here can recover — the host cannot be listened to for the
        // life of this panel — but it must not be silent about it. Without
        // this the panel simply never hears about an update, and looks
        // identical to a launch where there was none.
        failed: (reason) =>
          console.error("[nessa] the panel cannot hear the host about updates", reason),
      }),
    [],
  )

  const tab = updateTab(state)
  return {
    notice: updateNotice(state),
    tab,
    // A tab that is gone cannot be the one being looked at, however the
    // selection got there.
    viewing: selected && tab !== null,
    install: () => {
      report({ kind: "install" })
      setSelected(true)
      // A refusal comes back as an event, so the answer is not awaited here.
      // The rejection is still caught: an invoke that fails outright sends no
      // event, and dropping it would leave the tab saying it was downloading
      // with nothing ever arriving to correct it.
      void installUpdate().catch((reason) => {
        console.error("[nessa] the update could not be started", reason)
        report({ kind: "failed" })
      })
    },
    dismiss: () => report({ kind: "dismiss" }),
    close: () => {
      report({ kind: "closed" })
      setSelected(false)
    },
    setViewing: setSelected,
  }
}
