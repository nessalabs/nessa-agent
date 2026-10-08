// @vitest-environment jsdom
/**
 * The pieces every Settings tab is built from: a control is named by its
 * row and described by the row's line; a group none of whose settings is
 * available yet says so once, and every control it rests is described by
 * that note; a card named as its tab keeps the name for assistive
 * technology only.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import {
  Choices,
  Group,
  ItemRow,
  Row,
  Segmented,
  SettingGroup,
  Toggle,
} from "./settings-controls"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  Object.assign(globalThis, {
    ResizeObserver: class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  })
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const render = (node: React.ReactNode) => act(async () => root.render(node))
const noop = () => {}

/** The text an element's `aria-labelledby` / `aria-describedby` points at. */
function textOf(element: Element | null, attribute: string): string {
  return (element?.getAttribute(attribute) ?? "")
    .split(" ")
    .filter(Boolean)
    .map((id) => document.getElementById(id)?.textContent ?? `#${id}?`)
    .join(" | ")
}

describe("a row", () => {
  it("names its switch by the row's label and describes it by the row's line", async () => {
    await render(
      <Group>
        <Row id="drifting-light">
          <Toggle checked onChange={noop} />
        </Row>
      </Group>,
    )
    const toggle = host.querySelector('[role="switch"]')
    expect(textOf(toggle, "aria-labelledby")).toBe("Drifting light")
    expect(textOf(toggle, "aria-describedby")).toBe(
      "The light behind the window moves slowly.",
    )
    expect((toggle as HTMLButtonElement).disabled).toBe(false)
  })

  it("names a segmented choice by the row's label", async () => {
    await render(
      <Group>
        <Row id="animations">
          <Segmented
            value="on"
            onChange={noop}
            options={[
              { id: "on", label: "On" },
              { id: "off", label: "Off" },
            ]}
          />
        </Row>
      </Group>,
    )
    const group = host.querySelector('[data-slot="segmented-control"]')
    expect(textOf(group, "aria-labelledby")).toBe("Animations")
  })

  it("draws no line under a label that needs none", async () => {
    await render(
      <Group>
        <ItemRow label="Settings" control={<kbd>⌘,</kbd>} />
      </Group>,
    )
    expect(host.querySelector('[data-slot="settings-row-detail"]')).toBeNull()
  })
})

describe("a group of settings not available yet", () => {
  it("says so once, under its card, and rests every control, described by that note", async () => {
    await render(
      <Group>
        <Row id="open-at-login">
          <Toggle checked onChange={noop} />
        </Row>
        <Row id="menu-bar">
          <Toggle checked onChange={noop} />
        </Row>
        <Row id="window-opens-to">
          <Segmented
            value="home"
            onChange={noop}
            options={[
              { id: "home", label: "Home" },
              { id: "last", label: "Last session" },
            ]}
          />
        </Row>
      </Group>,
    )
    expect(host.textContent?.match(/Not available yet/g)).toHaveLength(1)
    const note = host.querySelector(".settings-unavailable")
    expect(note?.closest('[data-slot="settings-group-footnote"]')).not.toBeNull()
    const controls = host.querySelectorAll<HTMLElement>(
      '[role="switch"], [data-slot="segmented-control"]',
    )
    expect(controls).toHaveLength(3)
    for (const control of controls)
      expect(control.getAttribute("aria-describedby")).toContain(note?.id)
    expect(host.querySelectorAll("button:not(:disabled)")).toHaveLength(0)
  })

  it("says it on the row instead, when the group holds settings that are available", async () => {
    await render(
      <Group>
        <Row id="keep-sessions">
          <Toggle checked onChange={noop} />
        </Row>
        <Row id="drifting-light">
          <Toggle checked onChange={noop} />
        </Row>
      </Group>,
    )
    expect(host.querySelector(".settings-unavailable")).toBeNull()
    const pending = host.querySelector('[data-setting="keep-sessions"]')
    expect(pending?.textContent).toContain("Not available yet")
    expect(pending?.querySelector<HTMLButtonElement>('[role="switch"]')?.disabled).toBe(
      true,
    )
    const available = host.querySelector('[data-setting="drifting-light"]')
    expect(available?.textContent).not.toContain("Not available yet")
  })

  it("keeps its own note after the one that says it is not available", async () => {
    await render(
      <Group note="Only while the window is in the background or closed.">
        <Row id="notify-sound">
          <Toggle checked={false} onChange={noop} />
        </Row>
      </Group>,
    )
    expect(host.querySelector('[data-slot="settings-group-footnote"]')?.textContent).toBe(
      "Not available yet · Only while the window is in the background or closed.",
    )
  })
})

describe("a group's title", () => {
  it("is drawn only when given", async () => {
    await render(
      <>
        <Group title="Window">
          <ItemRow label="Settings" />
        </Group>
        <Group>
          <ItemRow label="New session" />
        </Group>
      </>,
    )
    expect(
      [...host.querySelectorAll(".settings-group > h2")].map((h) => h.textContent),
    ).toEqual(["Window"])
  })

  it("keeps a resting card's note on a listed row's controls, beside the row's own line", async () => {
    await render(
      <SettingGroup id="workspace-layout" pending>
        <ItemRow
          label="Codex"
          detail="Installed"
          control={<Toggle checked onChange={noop} />}
        />
      </SettingGroup>,
    )
    const note = host.querySelector(".settings-unavailable")
    expect(note).not.toBeNull()
    const described = textOf(host.querySelector('[role="switch"]'), "aria-describedby")
    expect(described).toBe(`${note?.textContent} | Installed`)
  })

  it("names a card of choices, and stays only for assistive technology when it repeats its tab", async () => {
    await render(
      <>
        <SettingGroup id="workspace-layout">
          <Choices
            options={[{ id: "columns", label: "Three columns" }]}
            value="columns"
            onChange={noop}
            art={() => null}
          />
        </SettingGroup>
        <SettingGroup id="icon-family">
          <Choices
            options={[{ id: "nessa", label: "Nessa" }]}
            value="nessa"
            onChange={noop}
            art={() => null}
          />
        </SettingGroup>
      </>,
    )
    const [layout, icons] = host.querySelectorAll(".settings-group")
    expect(layout.getAttribute("data-title")).toBe("hidden")
    expect(icons.getAttribute("data-title")).toBeNull()
    expect(textOf(layout.querySelector('[role="radiogroup"]'), "aria-labelledby")).toBe(
      "Layout",
    )
    expect(textOf(icons.querySelector('[role="radiogroup"]'), "aria-labelledby")).toBe(
      "Icons",
    )
  })
})
