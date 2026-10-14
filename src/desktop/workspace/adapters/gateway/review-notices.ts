/**
 * A permission review waiting on a person, as the gateway pushes it, and the
 * desktop notice that names it.
 *
 * The push is the whole set still waiting. A review that leaves the set is
 * cleared. Selecting the notice opens that conversation.
 */

/** One review the gateway says is waiting. */
export interface WaitingReview {
  readonly conversationId: string
  readonly executionId: string
  readonly permissionId: string
  readonly tool: string
}

/** Show a review, and close the notice when it is no longer waiting. */
export interface ReviewNotifier {
  show(review: WaitingReview): void
  clear(review: WaitingReview): void
}

const OPEN_REVIEW_EVENT = "nessa-open-review"

/** The window event a notice click sends, naming the conversation to open. */
export function openReviewEvent(conversationId: string): CustomEvent<string> {
  return new CustomEvent(OPEN_REVIEW_EVENT, { detail: conversationId })
}

/** Whether `event` is a notice click, and the conversation it names. */
export function reviewToOpen(event: Event): string | null {
  if (event.type !== OPEN_REVIEW_EVENT || !(event instanceof CustomEvent)) return null
  return typeof event.detail === "string" && event.detail.length > 0 ? event.detail : null
}

function text(record: object, name: string): string | null {
  if (!Object.hasOwn(record, name)) return null
  const value = (record as Record<string, unknown>)[name]
  return typeof value === "string" && value.length > 0 ? value : null
}

/** The reviews in a `permission.reviewWaiting` payload, or null when it is not one. */
export function waitingReviews(payload: unknown): readonly WaitingReview[] | null {
  if (
    typeof payload !== "object" ||
    payload === null ||
    !Object.hasOwn(payload, "reviews")
  )
    return null
  const reviews = (payload as { reviews: unknown }).reviews
  if (!Array.isArray(reviews)) return null
  const parsed: WaitingReview[] = []
  for (const review of reviews) {
    if (typeof review !== "object" || review === null) return null
    const conversationId = text(review, "conversationId")
    const executionId = text(review, "executionId")
    const permissionId = text(review, "permissionId")
    const tool = text(review, "tool")
    if (!conversationId || !executionId || !permissionId || !tool) return null
    parsed.push({ conversationId, executionId, permissionId, tool })
  }
  return parsed
}

const noticeKey = (review: WaitingReview) =>
  `${review.conversationId}\u0000${review.executionId}\u0000${review.permissionId}`

/**
 * Show each review that arrived and clear each one that left. A review still
 * in both sets is left as it is.
 */
export function applyReviewSnapshot(
  previous: readonly WaitingReview[],
  next: readonly WaitingReview[],
  notifier: ReviewNotifier,
): void {
  const staying = new Set(next.map(noticeKey))
  for (const review of previous) {
    if (!staying.has(noticeKey(review))) notifier.clear(review)
  }
  const shown = new Set(previous.map(noticeKey))
  for (const review of next) {
    if (!shown.has(noticeKey(review))) notifier.show(review)
  }
}

/**
 * The reviews still waiting, and a way to show them again.
 *
 * `apply` shows arrivals and clears departures. A review dropped because
 * notification permission was not granted is still in the last snapshot.
 * `showWaiting` shows that snapshot as if none of it had been shown, so
 * granting permission notifies for it. A notice the notifier already has
 * is left as it is.
 */
export function waitingReviewNotices(notifier: ReviewNotifier): {
  apply: (reviews: readonly WaitingReview[]) => void
  showWaiting: () => void
} {
  let previous: readonly WaitingReview[] = []
  let latest: readonly WaitingReview[] = []
  return {
    apply(reviews) {
      applyReviewSnapshot(previous, reviews, notifier)
      previous = reviews
      latest = reviews
    },
    showWaiting() {
      applyReviewSnapshot([], latest, notifier)
      previous = latest
    },
  }
}

let showWaitingReviewsNow: (() => void) | undefined

/** Point [`showWaitingReviews`] at the window's live waiting-review notices. */
export function bindWaitingReviewNotices(showWaiting: () => void): void {
  showWaitingReviewsNow = showWaiting
}

/**
 * Show the reviews that are waiting, including one that arrived while
 * notification permission was not granted. Settings calls this when the
 * person allows notifications. Nothing is registered until the desktop
 * window composes its gateway. A review already showing is not shown twice.
 */
export function showWaitingReviews(): void {
  showWaitingReviewsNow?.()
}

/**
 * The browser's notifications. A prompt that arrives with the page is a
 * WebKit console error, and one that arrives on the next click interrupts
 * that click, so this does not ask. Settings asks from its own button.
 * When permission is already granted, a review shows a notification. A
 * click on it asks `open` to bring the conversation up and focuses this
 * window. A denied permission shows nothing.
 */
export function browserReviewNotifier(
  open: (conversationId: string) => void,
): ReviewNotifier {
  const shown = new Map<string, Notification>()
  const post = (review: WaitingReview) => {
    const id = noticeKey(review)
    if (shown.has(id) || typeof Notification === "undefined") return
    if (Notification.permission !== "granted") return
    const notice = new Notification(`${review.tool} needs a decision`, {
      body: "A permission review is waiting.",
      tag: id,
    })
    notice.onclick = () => {
      open(review.conversationId)
      window.focus()
    }
    shown.set(id, notice)
  }
  return {
    show(review) {
      post(review)
    },
    clear(review) {
      const id = noticeKey(review)
      shown.get(id)?.close()
      shown.delete(id)
    },
  }
}
