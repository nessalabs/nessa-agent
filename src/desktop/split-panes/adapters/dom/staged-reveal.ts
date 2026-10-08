/** One adapter's pane bodies return one per frame, with cancellable ownership. */
export function stagedReveal(scope: HTMLElement, marker: string) {
  let active = true
  let frame: number | null = null
  let generation = 0
  const stop = () => {
    generation++
    if (frame !== null) cancelAnimationFrame(frame)
    frame = null
  }
  const step = (owner: number) => {
    if (!active || owner !== generation) return
    frame = null
    scope.querySelector(`[${marker}]`)?.removeAttribute(marker)
    if (scope.querySelector(`[${marker}]`))
      frame = requestAnimationFrame(() => step(owner))
  }
  return {
    stop,
    hold(panes: Iterable<Element> = scope.querySelectorAll("[data-pane-key]")) {
      if (!active) return
      stop()
      for (const pane of panes) if (scope.contains(pane)) pane.setAttribute(marker, "")
    },
    start() {
      if (!active || frame !== null || !scope.querySelector(`[${marker}]`)) return
      const owner = generation
      frame = requestAnimationFrame(() => step(owner))
    },
    dispose() {
      if (!active) return
      active = false
      stop()
      scope
        .querySelectorAll(`[${marker}]`)
        .forEach((pane) => pane.removeAttribute(marker))
    },
  }
}
