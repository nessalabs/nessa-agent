export const webdriverBudgets = Object.freeze({
  command: 10_000,
  session: 60_000,
})

const settle = async (start) => {
  try {
    return { ok: true, value: await start() }
  } catch (error) {
    return { ok: false, error }
  }
}

/** Own both observations required for a launched WebDriver session. */
export async function observeWebdriverStartup({ createSession, observeApplication }) {
  const controller = new AbortController()
  const session = settle(() => createSession(controller.signal))
  const application = settle(() => observeApplication(controller.signal))
  const labelled = [
    session.then((result) => ({ owner: "session", result })),
    application.then((result) => ({ owner: "application", result })),
  ]
  const first = await Promise.race(labelled)
  if (!first.result.ok) {
    controller.abort(first.result.error)
    await Promise.all([session, application])
    throw first.result.error
  }

  const second = await (first.owner === "session" ? application : session)
  if (!second.ok) {
    controller.abort(second.error)
    await Promise.all([session, application])
    throw second.error
  }

  return first.owner === "session"
    ? { session: first.result.value, application: second.value }
    : { session: second.value, application: first.result.value }
}

/** One labelled WebDriver request with a lifecycle-specific, bounded budget. */
export async function webdriverRequest({
  port,
  method,
  path,
  body,
  phase,
  lifecycle = "command",
  signal,
  fetchRequest = fetch,
  timeoutSignal = AbortSignal.timeout,
  now = performance.now.bind(performance),
}) {
  const timeout = webdriverBudgets[lifecycle]
  if (timeout === undefined) throw new Error(`unknown WebDriver lifecycle: ${lifecycle}`)
  const started = now()
  const signals = [timeoutSignal(timeout)]
  if (signal) signals.push(signal)
  try {
    const response = await fetchRequest(`http://127.0.0.1:${port}${path}`, {
      method,
      headers: body === undefined ? undefined : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.any(signals),
    })
    const text = await response.text()
    const result = text ? JSON.parse(text) : {}
    if (!response.ok || result.value?.error)
      throw new Error(`${method} ${path}: ${response.status} ${text}`)
    return result.value
  } catch (cause) {
    const elapsed = Math.max(0, Math.round(now() - started))
    throw new Error(
      `${phase} (${method} ${path}) failed after ${elapsed}ms ` +
        `[${lifecycle} budget ${timeout}ms]: ${cause}`,
      { cause },
    )
  }
}
