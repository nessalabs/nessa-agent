import { withCleanup } from "./browser.mjs"

/** Hold only the sample reply's timeline; real Playwright deadlines keep running. */
export async function withHeldSampleReply(page, run) {
  const clock = page.context().clock
  // Startup ran naturally. The lead is a fixture clock position, not a deadline.
  await clock.pauseAt(new Date(Date.now() + 60_000))
  return withCleanup(run, () => clock.resume())
}

/** Browser-side observer installed after the original fixture's message listener. */
export function captureTeardownRequest({ origin, binding }) {
  const owner = new AbortController()
  if (typeof window[binding] !== "function")
    throw new Error("the fixture observation binding was not installed before page boot")
  window.addEventListener(
    "message",
    (event) => {
      if (event.source !== window.parent || event.origin !== origin) return
      const request = event.data
      if (
        request === null ||
        typeof request !== "object" ||
        !Object.hasOwn(request, "jsonrpc") ||
        request.jsonrpc !== "2.0" ||
        !Object.hasOwn(request, "method") ||
        request.method !== "ui/resource-teardown" ||
        !Object.hasOwn(request, "id") ||
        !["string", "number"].includes(typeof request.id)
      )
        return
      // The original bubble listener already recorded this state synchronously.
      void window[binding]({
        kind: "request",
        origin: event.origin,
        id: request.id,
        state: document.body.getAttribute("data-fixture-state"),
        at: Date.now(),
      }).catch(() => {})
    },
    { signal: owner.signal },
  )
  return owner
}

/** Install before host boot: Window AT_TARGET listeners can run in registration order. */
export function captureTeardownAnswer({ binding, slot }) {
  if (window !== window.top) return
  const controller = new AbortController()
  let target
  const owner = {
    pin({ origin, frame, tabs }) {
      if (target) throw new Error("the teardown proxy is already captured")
      if (typeof window[binding] !== "function")
        throw new Error(
          "the fixture observation binding was not installed before page boot",
        )
      target = { origin, frame, tabs, proxy: frame.contentWindow }
      owner.initial = {
        tabs: document.querySelectorAll(tabs).length,
        connected: frame.isConnected,
        at: Date.now(),
      }
    },
    abort() {
      controller.abort()
    },
  }
  window.addEventListener(
    "message",
    (event) => {
      if (!target || event.source !== target.proxy || event.origin !== target.origin)
        return
      const answer = event.data
      if (
        answer === null ||
        typeof answer !== "object" ||
        !Object.hasOwn(answer, "jsonrpc") ||
        answer.jsonrpc !== "2.0" ||
        Object.hasOwn(answer, "method") ||
        !Object.hasOwn(answer, "id") ||
        !["string", "number"].includes(typeof answer.id)
      )
        return
      void window[binding]({
        kind: "answer",
        origin: event.origin,
        id: answer.id,
        ...(Object.hasOwn(answer, "result") ? { result: answer.result } : {}),
        ...(Object.hasOwn(answer, "error") ? { error: answer.error } : {}),
        tabs: document.querySelectorAll(target.tabs).length,
        connected: target.frame.isConnected,
        at: Date.now(),
        phase: event.eventPhase,
      }).catch(() => {})
    },
    { capture: true, signal: controller.signal },
  )
  // Only a fixture bootstrap rendezvous: observe adopts a JSHandle and deletes it.
  Object.defineProperty(window, slot, { value: owner, configurable: true })
  return owner
}

/** Install transport before opaque app documents boot; accept only later captured owners. */
export async function prepareFixtureTeardown(context) {
  const records = []
  const binding = "__mcpFixtureTeardown"
  const slot = "__mcpFixtureTeardownOwner"
  let app
  let parent
  let overflow = false
  let sawRequest
  const originalRequest = new Promise((resolve) => {
    sawRequest = resolve
  })
  await context.exposeBinding(binding, ({ frame }, record) => {
    if (record === null || typeof record !== "object" || !Object.hasOwn(record, "kind"))
      return
    if (
      (record.kind === "request" && frame === app) ||
      (record.kind === "answer" && frame === parent)
    ) {
      // One request and answer suffice; retain duplicates to fail, never grow indefinitely.
      if (records.length < 4) records.push(record)
      else overflow = true
      if (record.kind === "request") sawRequest(record)
    }
  })
  await context.addInitScript(captureTeardownAnswer, { binding, slot })
  return {
    async observe(page, frames, tabs) {
      app = frames.app
      parent = page.mainFrame()
      const owners = []
      let initial
      const close = async () => {
        const results = await Promise.allSettled(
          owners.map(async ([frame, handle]) => {
            await withCleanup(
              async () => {
                if (!frame.isDetached()) await handle.evaluate((owner) => owner.abort())
              },
              () => handle.dispose(),
            )
          }),
        )
        const errors = results
          .filter((result) => result.status === "rejected")
          .map((result) => result.reason)
        if (errors.length)
          throw new AggregateError(errors, "fixture observation cleanup failed")
      }
      try {
        const origin = new URL(frames.proxy.url()).origin
        owners.push([
          app,
          await app.evaluateHandle(captureTeardownRequest, { origin, binding }),
        ])
        const element = await frames.proxy.frameElement()
        await withCleanup(
          async () => {
            owners.push([
              parent,
              await page.evaluateHandle(
                ({ origin, frame, tabs, slot }) => {
                  const owner = Object.hasOwn(window, slot) ? window[slot] : undefined
                  delete window[slot]
                  if (!owner)
                    throw new Error(
                      "the teardown observer was not installed before host boot",
                    )
                  try {
                    owner.pin({ origin, frame, tabs })
                    return owner
                  } catch (error) {
                    owner.abort()
                    throw error
                  }
                },
                { origin, frame: element, tabs, slot },
              ),
            ])
          },
          () => element.dispose(),
        )
        initial = await owners[1][1].evaluate((owner) => owner.initial)
      } catch (error) {
        return withCleanup(async () => {
          throw error
        }, close)
      }
      return {
        records,
        initial,
        get overflow() {
          return overflow
        },
        async waitWithin(ms) {
          let timer
          try {
            return await Promise.race([
              originalRequest.then((request) => ({
                kind: "observed",
                withinMs: ms,
                request,
              })),
              new Promise((resolve) => {
                timer = setTimeout(() => resolve({ kind: "timed-out", withinMs: ms }), ms)
              }),
            ])
          } finally {
            clearTimeout(timer)
          }
        },
        close,
      }
    },
  }
}

/** A request and its exact answer must retain the tab until acknowledgement. */
export function teardownEvidence(records, tabs, overflow = false, initial) {
  const snapshot = structuredClone({ records, initial })
  const requests = snapshot.records.filter((record) => record.kind === "request")
  const request = requests[0]
  const validId =
    request &&
    Object.hasOwn(request, "id") &&
    ["string", "number"].includes(typeof request.id)
  const answers = snapshot.records.filter(
    (record) =>
      record.kind === "answer" &&
      validId &&
      Object.hasOwn(record, "id") &&
      record.id === request.id,
  )
  const answer = answers[0]
  const failures = []
  if (overflow) failures.push("the bounded teardown observation overflowed")
  if (
    requests.length !== 1 ||
    !validId ||
    !Object.hasOwn(request, "state") ||
    request.state !== "tearing-down"
  )
    failures.push(
      `the original app did not observe one teardown request while tearing-down: ${JSON.stringify(requests)}`,
    )
  if (
    answers.length !== 1 ||
    Object.hasOwn(answer, "error") ||
    !Object.hasOwn(answer, "result") ||
    JSON.stringify(answer.result) !== "{}"
  )
    failures.push(
      `the original proxy did not answer that teardown request once: ${JSON.stringify(answers)}`,
    )
  if (
    snapshot.initial &&
    (snapshot.initial.tabs !== tabs || snapshot.initial.connected !== true)
  )
    failures.push(
      `teardown observation did not begin on the original live tab: ${JSON.stringify(snapshot.initial)}`,
    )
  if (answer?.tabs !== tabs || answer?.connected !== true)
    failures.push(
      `the app's tab or original proxy closed before its answer: ${JSON.stringify({ expectedTabs: tabs, initial: snapshot.initial, answer })}`,
    )
  return {
    expectedTabs: tabs,
    initial: snapshot.initial,
    records: snapshot.records,
    request,
    answer,
    failures,
  }
}
