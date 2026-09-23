export const webdriverBudgets = Object.freeze({
  command: 10_000,
  session: 60_000,
})

export const nativePanelObservationScript = `
  const scalarPrefix = (value, maximum) => Array.from(String(value ?? '')).slice(0, maximum).join('');
  const element = (selector) => {
    const node = document.querySelector(selector);
    if (!node) return { present: false };
    const rect = node.getBoundingClientRect();
    const style = getComputedStyle(node);
    return { present: true, width: rect.width, height: rect.height,
      display: style.display, visibility: style.visibility };
  };
  const transcript = document.querySelector('[aria-label$=" transcript, 0 sent"]');
  const status = transcript?.querySelector('p.nessa-text-3');
  return {
    url: scalarPrefix(location.href, 1024),
    title: scalarPrefix(document.title, 256),
    readyState: document.readyState,
    surface: new URL(location.href).searchParams.get('surface'),
    root: element('[data-nessa-root]'),
    fallback: { ...element('[data-nessa-load-fallback]'),
      text: scalarPrefix(document.querySelector('[data-nessa-load-fallback]')?.textContent?.trim(), 512) },
    connectionText: status ? scalarPrefix(status.textContent?.trim(), 128) : null,
    bodyText: scalarPrefix(document.body?.innerText, 2048),
    viewport: { width: innerWidth, height: innerHeight },
  };`

/** Decide readiness from the observed page contract, independently of WebDriver. */
export function nativePanelReady(observation) {
  const root = observation?.root
  return Boolean(
    observation?.readyState === "complete" &&
    root?.present &&
    root.width > 0 &&
    root.height > 0 &&
    root.display !== "none" &&
    root.visibility !== "hidden" &&
    !observation?.fallback?.present &&
    observation?.connectionText === "Connected",
  )
}

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
