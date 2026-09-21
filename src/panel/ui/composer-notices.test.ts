// @vitest-environment jsdom
/**
 * The strip above the pill, at its worst.
 *
 * Five producers write here and none of them excludes the others, so the case
 * worth testing is all of them at once — the link, the update, both of the
 * attachment notices, the session, and the conversation. That is six cards,
 * which at the shipped 420px width is well over 400px of column above a
 * composer that does not shrink, against a window whose minimum height is
 * 320px. This file is about what happens to that column.
 *
 * What it can answer is what the DOM knows: that all six are still there, that
 * they are in the order this component declares rather than the order somebody
 * pasted them in, that every action is still in the tree and still in tab
 * order, and that a silent composer leaves nothing behind.
 *
 * What it cannot answer is the heights. jsdom does no layout: every box here
 * measures zero, so nothing in this file has ever observed the ceiling stop
 * anything. The ceiling is `.nessa-composer-notices` in `styles.css`, and the
 * part of it a test can hold is checked by `scripts/architecture` — that the
 * rule is still declared and still derived from the panel's own height. That
 * the 106px it comes to at a 320px window really does show a card and the top
 * of the next is arithmetic from the design system's tokens, not something any
 * of this watched happen.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

// The attachment notices read one pure rule from the conversation. Its barrel
// also exports the conversation's components, which need the whole design
// system resolved; `testing` is the same rules without them.
vi.mock("../../conversation", () => import("../../conversation/testing"))

import { AgentNotification } from "@nessa-ui/react/agent-notification"
import type { FileAttachment } from "../../conversation/testing"
import { AttachmentNotices } from "./attachment-notices"
import { ComposerNotices } from "./composer-notices"

const failedUpload: FileAttachment = {
  type: "file",
  id: "b",
  name: "b.png",
  mimeType: "image/png",
  size: 3,
  previewUrl: "blob:test",
  upload: { status: "failed", reason: "unavailable" },
}

const install = vi.fn()
const dismissLink = vi.fn()
const retryConversation = vi.fn()

/** Everything true at once: the six-card worst case, with its actions live. */
function worstCase() {
  return React.createElement(ComposerNotices, {
    link: React.createElement(AgentNotification, {
      className: "mb-2",
      state: "disconnected",
      title: "Link didn't open",
      description: "Nothing here could open that address.",
      dismissLabel: "Dismiss",
      onDismiss: dismissLink,
    }),
    update: React.createElement(AgentNotification, {
      className: "mb-2",
      state: "disconnected",
      title: "Update available",
      description: "9.9.9",
      retryLabel: "Install",
      onRetry: install,
    }),
    attachments: React.createElement(AttachmentNotices, {
      refusal: { reason: "file-too-large", names: ["holiday.mp4"] },
      files: [failedUpload],
      imageInput: true,
      onRetryUploads: () => {},
      onChooseFiles: () => {},
      onDismissRefusal: () => {},
    }),
    session: React.createElement(AgentNotification, {
      className: "mb-2",
      state: "disconnected",
      title: "Session needs attention",
      description: "The saved sign-in could not be restored.",
    }),
    conversation: React.createElement(AgentNotification, {
      className: "mb-2",
      state: "disconnected",
      title: "Message didn't send",
      retryLabel: "Retry message",
      onRetry: retryConversation,
    }),
  })
}

let container: HTMLDivElement
let root: Root

const region = () => container.querySelector<HTMLElement>(".nessa-composer-notices")!
/** The cards, top to bottom. */
const titles = () =>
  [...region().querySelectorAll('[data-slot="agent-notification"]')].map(
    (card) => card.querySelector("[aria-live] > div")?.textContent ?? "",
  )
const button = (label: string) =>
  region().querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom has no `matchMedia`, and a notification asks it about reduced motion
  // before it plays its exit.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
  vi.clearAllMocks()
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("says all six at once, in the order this component declares", async () => {
  await React.act(async () => {
    root.render(worstCase())
  })
  // The order is the component's, not the caller's: the caller names slots and
  // cannot stack them. Reordering the props above must not move a card.
  expect(titles()).toEqual([
    "Link didn't open",
    "Update available",
    "Image didn't upload",
    "File is too large",
    "Session needs attention",
    "Message didn't send",
  ])
})

it("keeps every action in the strip, and in tab order behind the strip's own stop", async () => {
  await React.act(async () => {
    root.render(worstCase())
  })
  // The ceiling scrolls; it drops nothing and collapses nothing. A Retry below
  // the fold is still in the tree, still enabled, and still reached by tabbing
  // — which is what scrolls it into view.
  for (const label of ["Dismiss", "Retry", "Install", "Retry message"]) {
    const action = button(label)
    expect(action, label).not.toBeNull()
    expect(action!.getAttribute("aria-disabled"), label).toBeNull()
    // `null` is the default stop, not a removal; only a card on its way out
    // takes itself out of the order.
    expect(action!.getAttribute("tabindex"), label).toBeNull()
  }
  await React.act(async () => {
    button("Install")!.click()
  })
  expect(install).toHaveBeenCalledOnce()

  // The scrolling box is itself a stop, because a card with nothing to press
  // has nothing inside it to tab to — so without this, a keyboard alone could
  // not scroll past one.
  expect(region().tabIndex).toBe(0)
  expect(region().getAttribute("role")).toBe("group")
  expect(region().getAttribute("aria-label")).toBe("Notices")
})

it("draws a ring on the stop it adds", async () => {
  await React.act(async () => {
    root.render(worstCase())
  })
  // Not a style test — jsdom has no stylesheet — but it does hold the one
  // pairing that silently makes the ring inert, which this shipped with once.
  // `outline-none` sets `--tw-outline-style: none`, and every `outline-*` width
  // utility draws with `outline-style: var(--tw-outline-style)`, so the two
  // together are a keyboard stop nobody can see when they reach it. The design
  // system's own scroller carries the pair; this must not copy it.
  const classes = [...region().classList]
  expect(classes).not.toContain("outline-none")
  expect(classes).toEqual(
    expect.arrayContaining([
      "focus-visible:outline-2",
      "focus-visible:outline-offset-2",
      "focus-visible:outline-ring",
    ]),
  )
})

it("leaves nothing behind when there is nothing to say", async () => {
  await React.act(async () => {
    root.render(
      React.createElement(ComposerNotices, {
        link: null,
        update: null,
        // The real component, saying nothing: it renders an empty fragment
        // rather than a node, which is what keeps the box empty.
        attachments: React.createElement(AttachmentNotices, {
          refusal: null,
          files: [],
          imageInput: true,
          onRetryUploads: () => {},
          onChooseFiles: () => {},
          onDismissRefusal: () => {},
        }),
        session: null,
        conversation: null,
      }),
    )
  })
  // `:empty` in styles.css takes the box off the screen, and with it the tab
  // stop and the gutter above the pill — but only if it really is empty. The
  // box adds no wrapper of its own, and the attachment notices, which are the
  // one slot that always renders something, render an empty fragment when they
  // have nothing to say. A child node of any kind here, including a stray
  // space, would leave a silent composer with a stop in it.
  expect(region().childNodes).toHaveLength(0)
})
