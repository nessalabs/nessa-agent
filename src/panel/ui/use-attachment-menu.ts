import * as React from "react"

/** Track the full composer's upper edge while its attachment menu is open. */
export function useAttachmentMenu() {
  const trigger = React.useRef<HTMLButtonElement>(null)
  const [open, setOpen] = React.useState(false)
  const [offset, setOffset] = React.useState(6)
  React.useLayoutEffect(() => {
    if (!open) return
    const button = trigger.current
    const composer = button?.closest('[data-slot="pill-composer"]')
    if (!button || !composer) return
    const measure = () =>
      setOffset(
        Math.max(
          0,
          button.getBoundingClientRect().top - composer.getBoundingClientRect().top,
        ) + 6,
      )
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(composer)
    observer.observe(button)
    return () => observer.disconnect()
  }, [open])
  return { trigger, open, setOpen, offset }
}
