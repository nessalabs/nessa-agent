// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { loadWorkspace, showContent } from "../../adapters/store/commands"
import { controlledAnimationFrames, testStore, type AnimationFrames } from "../../testing"
import { OverviewQuietProvider, useOverviewQuiet } from "./overview-quiet"

let frames: AnimationFrames
const mounted: { host: HTMLElement; react: Root }[] = []

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  frames = controlledAnimationFrames()
})
afterEach(async () => {
  for (const { host, react } of mounted.splice(0)) {
    await act(async () => react.unmount())
    host.remove()
  }
  frames.restore()
})

function Reader({ version }: { version: number }) {
  const quiet = useOverviewQuiet()
  return <output data-quiet={quiet} data-version={version} />
}

async function renderWindow() {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  const host = document.createElement("div")
  document.body.append(host)
  const react = createRoot(host)
  mounted.push({ host, react })
  const scope = { current: host }
  const render = (version = 0) =>
    act(async () =>
      react.render(
        <Provider store={store}>
          <OverviewQuietProvider store={store} root={scope}>
            <Reader version={version} />
          </OverviewQuietProvider>
        </Provider>,
      ),
    )
  await render()
  return {
    store,
    host,
    render,
    quiet: () => host.querySelector("output")?.getAttribute("data-quiet"),
    listed: () => {
      const marker = document.createElement("div")
      marker.setAttribute("data-overview-listed", "")
      host.append(marker)
    },
    open: () =>
      act(async () => {
        store.dispatch(showContent({ content: "agents" }))
      }),
  }
}
const paint = async (count = 3) => {
  for (let i = 0; i < count; i++) await act(async () => frames.runFrame())
}

it("subscribes each window to its own workspace store", async () => {
  const first = await renderWindow()
  const second = await renderWindow()
  await second.open()
  second.listed()
  await paint()
  expect(second.quiet()).toBe("true")
  expect(first.quiet()).toBe("false")
})

it("does not use another window's listed marker to publish this window", async () => {
  const other = document.createElement("div")
  other.setAttribute("data-overview-listed", "")
  document.body.append(other)
  try {
    const current = await renderWindow()
    await current.open()
    await paint(4)
    expect(current.quiet()).toBe("false")
    current.listed()
    await paint()
    expect(current.quiet()).toBe("true")
  } finally {
    other.remove()
  }
})

it("keeps delayed publication when its last reader rerenders", async () => {
  const current = await renderWindow()
  await current.open()
  await paint(1)
  await current.render(1)
  expect(current.quiet()).toBe("false")
  current.listed()
  await paint()
  expect(current.quiet()).toBe("true")
})

it("keeps its painted state when the window rerenders", async () => {
  const current = await renderWindow()
  await current.open()
  current.listed()
  await paint()
  expect(current.quiet()).toBe("true")
  await current.render(1)
  expect(current.quiet()).toBe("true")
})

it("finishes publication while unrelated store updates arrive between paints", async () => {
  const current = await renderWindow()
  await current.open()
  current.listed()
  for (let frame = 0; frame < 3; frame++) {
    await act(async () => current.store.dispatch({ type: "verification/unrelated" }))
    await paint(1)
  }
  expect(current.quiet()).toBe("true")
})
