import { useCallback, useEffect, useState } from "react"
import type { HostKind } from "../../../host/features"
import type { SettingsTabId } from "../model/settings-catalogue"
import { SettingsLayout } from "./settings-layout"
import { useSettingsSurface } from "./use-settings-surface"

/**
 * Prototype: Settings as its own surface over the window. A short sidebar of
 * categories and, beside it, the chosen category's page in one centred
 * column: its masthead (the name set large, and its tabs when it has
 * several), then the tab's settings in grouped rows. What exists is
 * catalogued in `model/settings-catalogue.ts`; the pages are in
 * `settings-tabs.tsx`; the pieces they are built from, over the UI kit's, in
 * `settings-controls.tsx`.
 *
 * Opened from "nessa Studio" at the foot of the app's sidebar (or ⌘,); in
 * Settings that same place reads "nessa Agent" with a back chevron, and
 * returns to the app. The sidebar toggle sits where the app's does and ⌘B
 * works the same; with the sidebar hidden, a back chevron beside the toggle
 * is the way out. Esc does not close Settings.
 *
 * This module is the way in. The orchestration is `useSettingsSurface`; the
 * markup is `SettingsLayout`.
 */
const openEvent = "nessa:open-settings"

/**
 * Opens Settings from anywhere in the window: on `tab` when one is named, as
 * where a sentence elsewhere sends the person ("Settings › Linked devices").
 */
export function openSettings(tab?: SettingsTabId) {
  window.dispatchEvent(
    new CustomEvent<SettingsTabId | undefined>(openEvent, { detail: tab }),
  )
}

interface SettingsHostProps {
  hostKind: HostKind
  browserSurface: boolean
}

/**
 * Whether Settings is open, and a way to close it: opened by `openSettings`
 * from anywhere, or ⌘,. The window holds this, so what lies under Settings
 * can be made inert while it is open.
 */
export function useSettingsOpening(): {
  open: boolean
  /** The tab it was opened on, when one was named. */
  tab: SettingsTabId | undefined
  close: () => void
} {
  const [open, setOpen] = useState(false)
  const [tab, setTab] = useState<SettingsTabId | undefined>(undefined)
  useEffect(() => {
    const show = (event: Event) => {
      setTab(
        event instanceof CustomEvent
          ? (event.detail as SettingsTabId | undefined)
          : undefined,
      )
      setOpen(true)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === ",") {
        event.preventDefault()
        setTab(undefined)
        setOpen(true)
      }
    }
    window.addEventListener(openEvent, show)
    window.addEventListener("keydown", onKeyDown)
    return () => {
      window.removeEventListener(openEvent, show)
      window.removeEventListener("keydown", onKeyDown)
    }
  }, [])
  const close = useCallback(() => setOpen(false), [])
  return { open, tab, close }
}

/**
 * Settings over the window. Modal: the window under it is inert while it is
 * open (the window does that, from `useSettingsOpening`), and focus goes back
 * to what opened it when it closes — or, gone meanwhile, to what is in
 * front (`focusInFront`).
 */
export function SettingsHost({
  open,
  onClose,
  ...props
}: SettingsHostProps & { open: boolean; tab?: SettingsTabId; onClose: () => void }) {
  return open ? <SettingsSurface {...props} onClose={onClose} /> : null
}

function SettingsSurface(
  props: SettingsHostProps & { tab?: SettingsTabId; onClose: () => void },
) {
  return <SettingsLayout {...useSettingsSurface(props)} />
}
