// @vitest-environment jsdom
/**
 * That the queue and the composer stay one element each, however often the
 * panel renders.
 *
 * They sit in one parent and were once keyed by the active conversation alone,
 * which made them one child under one key. React does not refuse that. Given a
 * sibling earlier in the same children array that renders nothing — the
 * composer has three, each a notice that is usually absent — it leaks one
 * subtree per render instead: the child array's fast path stops at the first
 * child producing no fiber, the remaining old fibers are matched by key, the
 * duplicate collides, and the loser is neither reused nor deleted.
 *
 * What that looked like was a composer filling with queue chips, one per
 * render, each frozen at the count it was born with, none removed when the
 * queue drained, and every one of them still there in the next conversation.
 *
 * The falsy sibling is the part worth keeping in the test. Without it the same
 * duplicate keys behave perfectly, which is why this survived review of the
 * change that introduced it and four passes over the code around it.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { act } from "react"
import { afterEach, beforeEach, describe, expect, it } from "vitest"

// React only treats `act` as real in an environment that claims to support it.
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let host: HTMLDivElement
let root: Root

beforeEach(() => {
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
})

/** A stand-in for the queue: one element, carrying what it was told. */
function Queue({ count }: { count: number }) {
  if (count <= 0) return null
  return React.createElement("button", { "data-slot": "queue" }, `Queued ${count}`)
}

function Composer() {
  return React.createElement("div", { "data-slot": "composer" })
}

/**
 * The composer's children in the order `app.tsx` renders them: notices that are
 * usually absent, then the queue, then the composer itself.
 */
function Panel({
  conversationId,
  queued,
  keyed,
}: {
  conversationId: string
  queued: number
  /** "shared" is what this file exists to refuse; "own" is what is shipped. */
  keyed: "shared" | "own"
}) {
  const queueKey = keyed === "shared" ? conversationId : `queue:${conversationId}`
  const composerKey = keyed === "shared" ? conversationId : `composer:${conversationId}`
  return React.createElement(
    "div",
    { className: "nessa-composer" },
    false,
    null,
    React.createElement(Queue, { key: queueKey, count: queued }),
    React.createElement(Composer, { key: composerKey }),
  )
}

function chips() {
  return host.querySelectorAll('[data-slot="queue"]')
}

function show(props: React.ComponentProps<typeof Panel>) {
  act(() => {
    root.render(React.createElement(Panel, props))
  })
}

describe("the composer's children", () => {
  it("draws one queue chip however many times the panel renders", () => {
    for (let render = 0; render < 8; render += 1) {
      show({ conversationId: "c0", queued: 3, keyed: "own" })
    }
    expect(chips()).toHaveLength(1)
    expect(chips()[0]?.textContent).toBe("Queued 3")
  })

  it("removes the chip when the queue drains", () => {
    show({ conversationId: "c0", queued: 2, keyed: "own" })
    show({ conversationId: "c0", queued: 0, keyed: "own" })
    expect(chips()).toHaveLength(0)
  })

  it("leaves no chip behind in the next conversation", () => {
    show({ conversationId: "c0", queued: 2, keyed: "own" })
    show({ conversationId: "c1", queued: 0, keyed: "own" })
    expect(chips()).toHaveLength(0)
  })

  it("would accumulate if the two shared one key, which is the defect", () => {
    // The guard for the guard: if React ever stopped leaking here, this test
    // would pass for the wrong reason and the one above would prove nothing.
    for (let render = 0; render < 8; render += 1) {
      show({ conversationId: "c0", queued: 3, keyed: "shared" })
    }
    expect(chips().length).toBeGreaterThan(1)
  })
})
