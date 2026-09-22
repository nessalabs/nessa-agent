export const webdriverBudgets = Object.freeze({
  command: 10_000,
  session: 60_000,
})

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
