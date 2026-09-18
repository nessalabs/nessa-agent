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

  React.useEffect(() => {
    let stale = false
    const subscriptions = [
      onUpdateAvailable((release) => {
        if (!stale) report({ kind: "announced", release })
      }),
      onUpdateProgress(({ downloaded, total }) => {
        if (!stale) report({ kind: "progress", downloaded, total })
      }),
      onUpdateFailed((reason) => {
        if (stale) return
        // The sentence on screen is the panel's; the host's own reason is
        // technical and belongs with the diagnostics, where a bug report can
        // find it. It is already on the host's stderr too.
        console.warn("[nessa] the update was not installed", reason)
        report({ kind: "failed" })
      }),
    ]
    // Asked after the subscriptions are in flight, so a check answering in
    // between is not lost between the two.
    void availableUpdate().then((release) => {
      if (!stale && release) report({ kind: "announced", release })
    })
    return () => {
      stale = true
      for (const subscription of subscriptions) {
        void subscription.then((unlisten) => unlisten())
      }
    }
  }, [])

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
      // A refusal comes back as an event, so nothing is awaited here.
      void installUpdate()
    },
    dismiss: () => report({ kind: "dismiss" }),
    close: () => {
      report({ kind: "closed" })
      setSelected(false)
    },
    setViewing: setSelected,
  }
}
