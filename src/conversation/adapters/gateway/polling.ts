/** One read at a time per mounted tab. Stop invalidates a late reply without closing shared work. */
export function pollConversation(
  read: () => Promise<unknown>,
  invalidate: () => void,
  delay: () => number = () => 250,
): () => void {
  let disposed = false
  let timer: ReturnType<typeof setTimeout> | undefined
  const poll = async () => {
    try {
      await read()
    } catch {
      /* The read command owns its surfaced error state. */
    } finally {
      if (!disposed)
        timer = setTimeout(() => {
          void poll()
        }, delay())
    }
  }
  void poll()
  return () => {
    disposed = true
    clearTimeout(timer)
    invalidate()
  }
}
