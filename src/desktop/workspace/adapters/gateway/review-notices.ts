/**
 * A permission review waiting on a person, as the gateway pushes it, and the
 * desktop notice that names it.
 *
 * The push is the whole set still waiting. A review that leaves the set is
 * cleared. The notice names the tool, and the conversation by its title when
 * the window has one, otherwise by its id. Selecting the notice opens that
 * conversation.
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

/** Whether notices can be shown, as Settings reports it. */
export type NoticeAccess = "unsupported" | "not-asked" | "allowed" | "denied"

/**
 * Where this window's notices are allowed. `ask` is Settings' Allow button,
 * and `blocked` is what Settings says when the person has denied them.
 */
export interface NoticeAccessSource {
  read(): Promise<NoticeAccess>
  ask(): Promise<NoticeAccess>
  readonly blocked: string
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

let readConversationTitle: (conversationId: string) => string | null = () => null
let showWaitingReviewsNow: (() => void) | undefined

/** What a notice says: the tool, and the conversation by title or id. */
function noticeText(review: WaitingReview): { title: string; body: string } {
  const name = readConversationTitle(review.conversationId)?.trim()
  return {
    title: `${review.tool} needs a decision`,
    body: name ? name : review.conversationId,
  }
}

const permissionAccess = (permission: NotificationPermission): NoticeAccess =>
  permission === "granted" ? "allowed" : permission === "denied" ? "denied" : "not-asked"

/**
 * The browser's notification permission. A prompt that arrives with the page
 * is a WebKit console error, and one that arrives on the next click
 * interrupts that click, so only `ask`, from Settings' own button, asks.
 */
export const browserNoticeAccess: NoticeAccessSource = {
  read: () =>
    Promise.resolve(
      typeof Notification === "undefined"
        ? "unsupported"
        : permissionAccess(Notification.permission),
    ),
  ask: () =>
    typeof Notification === "undefined"
      ? Promise.resolve("unsupported")
      : Notification.requestPermission().then(permissionAccess),
  blocked: "Blocked in this browser. Allow notifications for this site to turn them on.",
}

let noticeAccessNow: NoticeAccessSource = browserNoticeAccess

/** Point Settings at where this window's notices are allowed. */
export function bindNoticeAccess(source: NoticeAccessSource): void {
  noticeAccessNow = source
}

/** Where this window's notices are allowed: the browser's, until composition binds the host's. */
export function noticeAccess(): NoticeAccessSource {
  return noticeAccessNow
}

/** Point a waiting-review notice at the conversation's title, read when it is shown. */
export function bindConversationTitles(
  read: (conversationId: string) => string | null,
): void {
  readConversationTitle = read
}

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
    const { title, body } = noticeText(review)
    const notice = new Notification(title, { body, tag: id })
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

/** A waiting review and what its notice says. */
export interface WaitingNotice extends WaitingReview {
  readonly title: string
  readonly body: string
}

/** The host's review notices: the system's notification center. */
export interface HostNotices {
  /** Make the notices that are up the ones for this whole waiting set. */
  sync(notices: readonly WaitingNotice[]): Promise<void>
}

/**
 * Notices the desktop host shows, for a webview with no notifications of its
 * own (WKWebView has none; WebKitGTK denies them). The host keeps which
 * notices are up and diffs each set against them, so this sends every set
 * whole, the empty one included, and a reloaded window cannot leave one
 * behind. One set is in flight at a time and only the newest waiting set
 * follows it: an older set landing after a newer one would put back a
 * review that settled. `showWaiting` sends the newest set again, so the
 * host retries what the system refused before (permission not yet granted).
 */
export function hostReviewNotices(host: HostNotices): {
  apply: (reviews: readonly WaitingReview[]) => void
  showWaiting: () => void
} {
  let latest: readonly WaitingReview[] = []
  let sending = false
  let queued = false
  const send = () => {
    if (sending) {
      queued = true
      return
    }
    sending = true
    queued = false
    const notices = latest.map((review) => ({ ...review, ...noticeText(review) }))
    void host
      .sync(notices)
      .catch((cause: unknown) => {
        console.warn("[nessa] review notices were not updated", cause)
      })
      .finally(() => {
        sending = false
        if (queued) send()
      })
  }
  return {
    apply(reviews) {
      latest = reviews
      send()
    },
    showWaiting() {
      send()
    },
  }
}
