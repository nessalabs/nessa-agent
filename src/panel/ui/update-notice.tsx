import { Download, X } from "lucide-react"

import type { UpdateNotice as Notice } from "../application/update-surface"

/**
 * An available update, above the composer.
 *
 * Two words, a version, and two controls that do opposite things. It carries no
 * prose because there is nothing to explain: one control takes the update and
 * the other makes the notice go away, and the tab that opens is where anything
 * worth reading about the release actually is.
 *
 * The controls are icon-only, so their `aria-label`s are the only names they
 * have — and both name the version, because "Install" and "Dismiss" alone would
 * tell somebody using a screen reader less than the notice tells everybody
 * else. The glyphs are hidden from the accessibility tree, as they are in
 * setup's chrome.
 */
export function UpdateNotice({
  notice,
  onInstall,
  onDismiss,
}: {
  notice: Notice
  onInstall: () => void
  onDismiss: () => void
}) {
  return (
    <div className="nessa-update-notice">
      {/* Announced when it appears: it arrives unasked, part-way through
          whatever somebody was doing. */}
      <div className="nessa-update-notice-text" role="status">
        <strong>Update available</strong>
        <span>{notice.version}</span>
      </div>
      <div className="nessa-update-notice-controls">
        <button
          type="button"
          aria-label={notice.installLabel}
          title={notice.installLabel}
          onClick={onInstall}
          className="nessa-update-control nessa-update-control-primary"
        >
          <Download aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label={notice.dismissLabel}
          title={notice.dismissLabel}
          onClick={onDismiss}
          className="nessa-update-control"
        >
          <X aria-hidden="true" />
        </button>
      </div>
    </div>
  )
}
