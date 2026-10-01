/**
 * Composition registers the sample widget plugin only beside the sample
 * workspace, whose session its widgets belong to; a window on another source
 * starts with the plugins it is given, and two under one id stop it.
 */
import { describe, expect, it } from "vitest"
import { createDesktopDependencies } from "./dependencies"
import { samplePlugin, samplePluginId, WidgetRegistryError } from "./widgets"
import { fakeSource } from "./workspace/testing"

describe("the window's widget plugins", () => {
  it("are the sample plugin's while the sample workspace is in use", () => {
    const { widgets } = createDesktopDependencies()
    expect(widgets.natives().map((plugin) => plugin.id)).toEqual([samplePluginId])
  })

  it("are none of the samples' on another source", () => {
    const { widgets } = createDesktopDependencies({ workspace: fakeSource() })
    expect(widgets.natives()).toEqual([])
    expect(widgets.plugin(samplePluginId)).toBeUndefined()
  })

  it("are the ones composition names, and two under one id stop the window", () => {
    const named = samplePlugin("a")
    const { widgets } = createDesktopDependencies({
      workspace: fakeSource(),
      widgets: [named],
    })
    expect(widgets.natives()).toEqual([named])
    expect(() =>
      createDesktopDependencies({ workspace: fakeSource(), widgets: [named, named] }),
    ).toThrow(WidgetRegistryError)
  })
})
