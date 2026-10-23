import { EventEmitter } from "node:events"

export function navigationObserver() {
  const observer = new EventEmitter()
  observer.detached = false
  observer.send = async () => ({ frameTree: { frame: { id: "main", loaderId: "old" } } })
  observer.detach = async () => {
    observer.detached = true
  }
  return observer
}

// Supplies the same error-document ordering to tests whose concern is elsewhere.
export function navigationPage(goto) {
  let observer
  return {
    context: () => ({ newCDPSession: async () => (observer = navigationObserver()) }),
    goto: async (url, options) => {
      try {
        return await goto(url, options)
      } catch (error) {
        observer.emit("Page.frameNavigated", {
          frame: {
            id: "main",
            loaderId: "failed",
            url: "chrome-error://chromewebdata/",
            unreachableUrl: url,
          },
        })
        observer.emit("Page.lifecycleEvent", {
          frameId: "main",
          loaderId: "failed",
          name: "DOMContentLoaded",
        })
        throw error
      }
    },
  }
}
