import { UPDATE_TAB_ID, type UpdateTab as Tab } from "../application/update-surface"

/**
 * The update, once somebody has taken it: where it is going, what the release
 * said, and how far the download has got.
 *
 * It restarts the app by itself when the download completes, so there is no
 * second confirmation here and no control to press. The one thing that can need
 * a decision is a refusal, and that replaces the bar with a sentence and a
 * retry.
 *
 * The content is a card rather than text laid straight onto the tab. The panel
 * has a transparent surface mode in which it is only a tint over the desktop
 * (see `styles.css`), and `--foreground` over somebody's wallpaper is not text.
 * Everything in the panel that has to stay readable there carries its own
 * `--card` surface — the notice above the composer, the detail sheet, the
 * active tab pill — so the tab does the same thing rather than a new one.
 *
 * The card is centred in the tab and its foot holds the state, so the four
 * things this tab can be showing — downloading with notes, downloading without
 * them, refused, and the moment the bar is full — are the same shape with
 * different contents, rather than a page that stops a fifth of the way down.
 *
 * The bar is a `progressbar` with its values on it while the server declared a
 * length, and a bar with no value at all — which is what ARIA means by an
 * indeterminate one — when it did not. A live region was the alternative and is
 * worse: a percentage that changes a hundred times would be announced a hundred
 * times, over whatever else is being read. The two endings are the parts worth
 * interrupting for, and only the refusal is one this tab shows — success is the
 * app restarting.
 */
export function UpdateTab({ tab, onRetry }: { tab: Tab; onRetry: () => void }) {
  const measured = tab.progress.kind === "downloading" ? tab.progress.percent : null
  return (
    <div
      className="nessa-update-tab"
      role="tabpanel"
      id={`chat-tab-panel-${UPDATE_TAB_ID}`}
      aria-label={`Update ${tab.to}`}
    >
      <div className="nessa-update-pane">
        <p className="nessa-update-versions">
          {tab.from} <span aria-hidden="true">→</span>
          <span className="sr-only">to</span> {tab.to}
        </p>
        <p className="nessa-update-headline">{tab.headline}</p>
        {tab.notes.title !== null && (
          <p className="nessa-update-release-title">{tab.notes.title}</p>
        )}
        {tab.notes.points.length > 0 && (
          <ul className="nessa-update-notes">
            {tab.notes.points.map((point) => (
              <li key={point}>{point}</li>
            ))}
          </ul>
        )}
        {tab.notes.empty && <p className="nessa-update-empty">{tab.notes.empty}</p>}

        <div className="nessa-update-foot">
          {tab.progress.kind === "failed" ? (
            <div className="nessa-update-refused">
              {/* Somebody asked for this and is waiting on it, so the answer
                  interrupts rather than waiting to be noticed. */}
              <p role="alert">{tab.progress.statement}</p>
              <button
                type="button"
                onClick={onRetry}
                aria-label={tab.progress.retryLabel}
                className="nessa-update-retry"
              >
                Retry
              </button>
            </div>
          ) : (
            <>
              <div
                className="nessa-update-track"
                role="progressbar"
                aria-label={`Downloading update ${tab.to}`}
                aria-valuemin={measured === null ? undefined : 0}
                aria-valuemax={measured === null ? undefined : 100}
                aria-valuenow={measured === null ? undefined : measured}
                aria-valuetext={measured === null ? undefined : `${measured}%`}
              >
                <div
                  className="nessa-update-fill"
                  data-unmeasured={measured === null || undefined}
                  style={measured === null ? undefined : { width: `${measured}%` }}
                />
              </div>
              <p className="nessa-update-progress-line">
                <span>{tab.progress.label}</span>
                {measured !== null && (
                  <span className="nessa-update-percent">{measured}%</span>
                )}
              </p>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
