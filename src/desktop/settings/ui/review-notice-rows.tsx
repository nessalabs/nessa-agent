import { useEffect, useState } from "react"
import {
  noticeAccess,
  showWaitingReviews,
  type NoticeAccess,
} from "../../workspace/adapters/gateway/review-notices"
import { setting } from "../model/settings-catalogue"
import { Group, ItemRow } from "./settings-controls"

/**
 * Settings › Privacy & Permissions › Access: the button that asks for
 * permission to show review notices. In the app the host asks the system;
 * in a browser preview the browser asks. Either way the prompt has to be
 * this press: a browser prompt that arrives with the page is a WebKit
 * console error, and one that arrives on the next click interrupts that
 * click. When permission is already granted, a waiting review shows a
 * notification on its own. Granting it from Allow also shows a review that
 * is already waiting.
 */
export function ReviewNoticeRows() {
  const source = noticeAccess()
  const [access, setAccess] = useState<NoticeAccess>("unsupported")
  useEffect(() => {
    let live = true
    void source
      .read()
      .catch(() => "unsupported" as const)
      .then((read) => {
        if (live) setAccess(read)
      })
    return () => {
      live = false
    }
  }, [source])
  if (access === "unsupported") return null
  const entry = setting("review-notices")
  const detail =
    access === "allowed"
      ? "On. A review that is waiting shows a notification."
      : access === "denied"
        ? source.blocked
        : entry.detail
  return (
    <Group title={entry.label}>
      <ItemRow
        setting="review-notices"
        label={entry.label}
        detail={detail}
        unavailable={access === "denied"}
        control={
          access === "not-asked" ? (
            <button
              type="button"
              onClick={() => {
                void source
                  .ask()
                  // Not asked after all: the button stays for another try.
                  .catch(() => "not-asked" as const)
                  .then((next) => {
                    setAccess(next)
                    if (next === "allowed") showWaitingReviews()
                  })
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
