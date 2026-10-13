import { useState } from "react"
import { setting } from "../model/settings-catalogue"
import { Group, ItemRow } from "./settings-controls"

/**
 * Settings › Privacy & Permissions › Access: the button that asks the
 * browser for notification permission. The prompt has to be this press.
 * A prompt that arrives with the page is a WebKit console error, and one
 * that arrives on the next click interrupts that click. When permission
 * is already granted, a waiting review shows a notification on its own.
 */
export function ReviewNoticeRows() {
  const supported = typeof Notification !== "undefined"
  const [permission, setPermission] = useState<NotificationPermission | "unsupported">(
    supported ? Notification.permission : "unsupported",
  )
  if (permission === "unsupported") return null
  const entry = setting("review-notices")
  const detail =
    permission === "granted"
      ? "On. A review that is waiting shows a notification."
      : permission === "denied"
        ? "Blocked in this browser. Allow notifications for this site to turn them on."
        : entry.detail
  return (
    <Group title={entry.label}>
      <ItemRow
        setting="review-notices"
        label={entry.label}
        detail={detail}
        unavailable={permission === "denied"}
        control={
          permission === "default" ? (
            <button
              type="button"
              onClick={() => {
                void Notification.requestPermission().then(setPermission)
              }}
            >
              Allow
            </button>
          ) : undefined
        }
      />
    </Group>
  )
}
