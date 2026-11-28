// @vitest-environment jsdom
/**
 * The composer shows the model its owner gives it. Its Return: a press sends
 * what is typed; a held Return sends it once. Its repeats — carried in from an approval answered by the same press,
 * whose card went and left the caret here — send nothing.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { Composer, type ComposerImages } from "./composer"

let host: HTMLDivElement

const appModeKey = "nessa.desktop.app-mode"
const advanced = JSON.stringify({ mode: "advanced", picks: {} })

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // The full row, as Advanced shows it; Simple's + is tested on its own below.
  localStorage.setItem(appModeKey, advanced)
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => {
  host.remove()
  localStorage.removeItem(appModeKey)
})

it("does not measure an empty draft, closes its page, and still measures nonempty drafts", async () => {
  const root = createRoot(host)
  const changes: boolean[] = []
  const sheet = document.createElement("style")
  sheet.textContent = ".desktop-composer textarea { line-height: 24px; padding: 0; }"
  document.head.append(sheet)
  const before = Object.getOwnPropertyDescriptor(
    HTMLTextAreaElement.prototype,
    "scrollHeight",
  )
  let reads = 0
  Object.defineProperty(HTMLTextAreaElement.prototype, "scrollHeight", {
    configurable: true,
    get() {
      reads++
      return 8 * 24
    },
  })
  const render = (page: boolean, text: string) =>
    act(async () =>
      root.render(
        <Composer
          page={page}
          onPageChange={(next) => changes.push(next)}
          text={text}
          onTextChange={() => {}}
          placeholder={"A wrapping placeholder ".repeat(20)}
          model={undefined}
          onModelChange={() => {}}
        />,
      ),
    )
  try {
    await render(false, "")
    expect(reads).toBe(0)
    expect(changes).toEqual([])
    await render(true, "")
    expect(reads).toBe(0)
    expect(changes).toEqual([false])
    await render(false, Array.from({ length: 8 }, () => "draft line").join("\n"))
    expect(reads).toBeGreaterThan(0)
    expect(changes).toEqual([false, true])
  } finally {
    await act(async () => root.unmount())
    sheet.remove()
    if (before)
      Object.defineProperty(HTMLTextAreaElement.prototype, "scrollHeight", before)
    else Reflect.deleteProperty(HTMLTextAreaElement.prototype, "scrollHeight")
  }
})

it("sends on a press of Return, and nothing on its repeats", async () => {
  const sent: string[] = []
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Composer
        page={false}
        onPageChange={() => {}}
        onSend={(text) => sent.push(text)}
        text="ship it"
        onTextChange={() => {}}
        model={undefined}
        onModelChange={() => {}}
      />,
    ),
  )
  const field = host.querySelector("textarea")
  if (!field) throw new Error("no textarea")
  const enter = (repeat: boolean) =>
    act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          code: "Enter",
          repeat,
          bubbles: true,
          cancelable: true,
        }),
      )
    })
  await enter(true)
  expect(sent).toEqual([])
  await enter(false)
  expect(sent).toEqual(["ship it"])
  await act(async () => root.unmount())
})

it("offers a paste of images to its owner, shows each as a tile it can take off, and sends images alone", async () => {
  const pasted: File[][] = []
  const removed: string[] = []
  const sent: string[] = []
  const root = createRoot(host)
  const render = (images: ComposerImages["images"]) =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          onSend={(text) => sent.push(text)}
          text=""
          onTextChange={() => {}}
          model={undefined}
          onModelChange={() => {}}
          images={{
            images,
            onPaste: (files) => pasted.push(files),
            onRemove: (id) => removed.push(id),
          }}
        />,
      ),
    )
  await render([])
  const field = host.querySelector("textarea")
  const send = host.querySelector<HTMLButtonElement>('button[aria-label="Send"]')
  if (!field || !send) throw new Error("no composer")
  expect(send.disabled).toBe(true)
  const shot = new File([new Uint8Array([1])], "image.png", { type: "image/png" })
  const paste = new Event("paste", { bubbles: true, cancelable: true })
  Object.defineProperty(paste, "clipboardData", {
    value: { getData: () => "", files: [shot], items: [] },
  })
  await act(async () => {
    field.dispatchEvent(paste)
  })
  expect(pasted).toEqual([[shot]])
  // The field inserts nothing of its own for it.
  expect(paste.defaultPrevented).toBe(true)
  await render([
    {
      id: "i",
      kind: "image",
      name: "image.png",
      mimeType: "image/png",
      size: 1,
      url: "blob:shot",
    },
  ])
  expect(host.querySelector(".desktop-composer-images img")?.getAttribute("src")).toBe(
    "blob:shot",
  )
  expect(send.disabled).toBe(false)
  await act(async () => send.click())
  expect(sent).toEqual([""])
  await act(async () =>
    host
      .querySelector<HTMLButtonElement>('button[aria-label="Remove image.png"]')
      ?.click(),
  )
  expect(removed).toEqual(["i"])
  await act(async () => root.unmount())
})

it("offers files picked with Attach to its owner, and shows a file by its name, not a picture", async () => {
  const picked: File[][] = []
  const root = createRoot(host)
  const render = (images: ComposerImages["images"]) =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          onSend={() => {}}
          text=""
          onTextChange={() => {}}
          model={undefined}
          onModelChange={() => {}}
          images={{ images, onPaste: (files) => picked.push(files), onRemove: () => {} }}
        />,
      ),
    )
  await render([])
  const input = host.querySelector<HTMLInputElement>('input[type="file"]')
  const attach = host.querySelector<HTMLButtonElement>(
    'button[aria-label="Attach files"]',
  )
  if (!input || !attach) throw new Error("no Attach")
  expect(input.multiple).toBe(true)
  let opened = 0
  input.addEventListener("click", () => opened++)
  await act(async () => attach.click())
  expect(opened).toBe(1)
  const pdf = new File(["%PDF"], "plan.pdf", { type: "application/pdf" })
  Object.defineProperty(input, "files", { value: [pdf], configurable: true })
  await act(async () => {
    input.dispatchEvent(new Event("change", { bubbles: true }))
  })
  expect(picked).toEqual([[pdf]])
  await render([
    {
      id: "f",
      kind: "file",
      name: "plan.pdf",
      mimeType: "application/pdf",
      size: 4,
      url: "blob:pdf",
    },
  ])
  const tile = host.querySelector('.desktop-composer-image[data-kind="file"]')
  expect(tile?.textContent).toContain("plan.pdf")
  expect(tile?.querySelector("img")).toBeNull()
  await act(async () => root.unmount())
})

it("offers no Attach without an owner for attachments", async () => {
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Composer
        page={false}
        onPageChange={() => {}}
        text=""
        onTextChange={() => {}}
        model={undefined}
      />,
    ),
  )
  expect(host.querySelector('button[aria-label="Attach files"]')).toBeNull()
  await act(async () => root.unmount())
})

it("leaves a paste of text to the field", async () => {
  const pasted: File[][] = []
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Composer
        page={false}
        onPageChange={() => {}}
        text=""
        onTextChange={() => {}}
        model={undefined}
        onModelChange={() => {}}
        images={{
          images: [],
          onPaste: (files) => pasted.push(files),
          onRemove: () => {},
        }}
      />,
    ),
  )
  const field = host.querySelector("textarea")
  if (!field) throw new Error("no textarea")
  const paste = new Event("paste", { bubbles: true, cancelable: true })
  Object.defineProperty(paste, "clipboardData", {
    value: { getData: () => "cells", files: [new File(["x"], "cells.png")], items: [] },
  })
  await act(async () => {
    field.dispatchEvent(paste)
  })
  expect(pasted).toEqual([])
  expect(paste.defaultPrevented).toBe(false)
  await act(async () => root.unmount())
})

it("shows Stop in place of Send while given one, and holds it while a stop is asked", async () => {
  let stops = 0
  const root = createRoot(host)
  const render = (stopping: boolean) =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          onSend={() => {}}
          onStop={{ stop: () => stops++, stopping }}
          text=""
          onTextChange={() => {}}
          model={undefined}
          onModelChange={() => {}}
        />,
      ),
    )
  await render(false)
  const button = () => host.querySelector<HTMLButtonElement>(".desktop-send")
  expect(host.querySelector('[aria-label="Send"]')).toBeNull()
  expect(button()?.getAttribute("aria-label")).toBe("Stop")
  // A Stop is a button of its own, not the form's submit.
  expect(button()?.type).toBe("button")
  await act(async () => button()?.click())
  expect(stops).toBe(1)
  await render(true)
  expect(button()?.disabled).toBe(true)
  await act(async () => root.unmount())
})

it("shows the model it is given, and only that", async () => {
  const root = createRoot(host)
  const render = (model: { provider: string; modelId: string } | undefined) =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          text=""
          onTextChange={() => {}}
          model={model}
          onModelChange={() => {}}
        />,
      ),
    )
  const shown = () =>
    host.querySelector(".desktop-chip-model")?.getAttribute("aria-label") ?? ""
  await render({ provider: "anthropic", modelId: "claude-opus-5" })
  expect(shown()).toContain("Claude Opus 5")
  await render({ provider: "openai", modelId: "gpt-6-astra" })
  expect(shown()).toContain("GPT-6 Astra")
  // A model the catalogue does not have is shown as none chosen, never as another.
  await render({ provider: "openai", modelId: "not-in-the-catalogue" })
  expect(host.querySelector(".desktop-chip-model")?.textContent).toContain("Choose model")
  await act(async () => root.unmount())
})

it("offers Fast and the levels only where the catalogue records them for the model", async () => {
  const root = createRoot(host)
  const render = (modelId: string) =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          text=""
          onTextChange={() => {}}
          model={{ provider: "anthropic", modelId }}
          onModelChange={() => {}}
        />,
      ),
    )
  const fast = () => host.querySelector<HTMLButtonElement>(".desktop-composer-fast")
  const level = () =>
    host.querySelector(
      '.desktop-chip-model [data-slot="model-level-picker-trigger-level"]',
    )?.textContent ?? null
  // Claude Opus 5 publishes Fast; Claude Sonnet 5 does not; Claude Haiku 4.5
  // reasons with no level recorded (crates/nessa-sdk/data/models.json).
  await render("claude-opus-5")
  expect(fast()?.disabled).toBe(false)
  expect(level()).toBe("Medium")
  await act(async () => fast()?.click())
  expect(fast()?.getAttribute("aria-pressed")).toBe("true")
  await render("claude-sonnet-5")
  expect(fast()?.disabled).toBe(true)
  expect(fast()?.getAttribute("aria-pressed")).toBe("false")
  expect(level()).toBe("Medium")
  await render("claude-haiku-4-5-20251001")
  expect(level()).toBeNull()
  // Fast is remembered for the next model that has it.
  await render("claude-opus-5")
  expect(fast()?.getAttribute("aria-pressed")).toBe("true")
  await act(async () => root.unmount())
})

it("chooses a model and its level with one press of a bar, which keeps the picker open, and a name keeps the level", async () => {
  const root = createRoot(host)
  const picks: { provider: string; modelId: string }[] = []
  let model = { provider: "anthropic", modelId: "claude-opus-5" }
  const render = () =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          text=""
          onTextChange={() => {}}
          model={model}
          onModelChange={(next) => {
            picks.push(next)
            model = next
          }}
        />,
      ),
    )
  await render()
  const chip = () => host.querySelector<HTMLButtonElement>(".desktop-chip-model")
  const dialog = () => document.querySelector('[data-slot="model-level-picker"]')
  try {
    await act(async () => chip()?.click())
    expect(dialog()).not.toBeNull()
    await act(async () =>
      document.querySelector<HTMLButtonElement>('[aria-label="All models"]')?.click(),
    )
    await act(async () =>
      document
        .querySelector<HTMLButtonElement>('[aria-label="Claude Sonnet 5, High"]')
        ?.click(),
    )
    await render()
    expect(picks).toEqual([{ provider: "anthropic", modelId: "claude-sonnet-5" }])
    // A bar leaves the picker open, so another level can be tried.
    expect(dialog()).not.toBeNull()
    expect(chip()?.getAttribute("aria-label")).toBe(
      "Change model, currently Claude Sonnet 5, High",
    )
    // GPT-5.6 Sol lists the same High, so choosing it by name keeps it.
    await act(async () =>
      document
        .querySelector<HTMLElement>(
          '[data-slot="model-level-picker-row"][data-model="gpt-5.6-sol"]',
        )
        ?.click(),
    )
    await render()
    expect(dialog()).toBeNull()
    expect(chip()?.getAttribute("aria-label")).toBe(
      "Change model, currently GPT-5.6 Sol, High",
    )
    // Choosing the model shown is passed on too: it keeps that model while
    // another client's change is still arriving.
    await act(async () => chip()?.click())
    await act(async () =>
      document.querySelector<HTMLButtonElement>('[aria-label="All models"]')?.click(),
    )
    await act(async () =>
      document
        .querySelector<HTMLElement>(
          '[data-slot="model-level-picker-row"][data-model="gpt-5.6-sol"]',
        )
        ?.click(),
    )
    expect(picks.at(-1)).toEqual({ provider: "openai", modelId: "gpt-5.6-sol" })
    expect(picks).toHaveLength(3)
  } finally {
    await act(async () => root.unmount())
  }
})

it("keeps the model and Fast in its row, and offers no project there", async () => {
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Composer
        page={false}
        onPageChange={() => {}}
        text=""
        onTextChange={() => {}}
        placeholder="Reply"
        model={undefined}
        onModelChange={() => {}}
      />,
    ),
  )
  try {
    const row = host.querySelector(".desktop-composer-row")
    expect(row?.querySelector(".desktop-chip-model")).not.toBeNull()
    expect(row?.querySelector(".desktop-composer-fast")).not.toBeNull()
    expect(host.querySelector('[aria-label^="Project"]')).toBeNull()
  } finally {
    await act(async () => root.unmount())
  }
})

it("marks itself empty until anything is typed or attached, and its circle takes focus", async () => {
  const root = createRoot(host)
  const render = (text: string, images: ComposerImages["images"] = []) =>
    act(async () =>
      root.render(
        <Composer
          page={false}
          onPageChange={() => {}}
          text={text}
          onTextChange={() => {}}
          model={{ provider: "anthropic", modelId: "claude-opus-5" }}
          onModelChange={() => {}}
          images={{ images, onPaste: () => {}, onRemove: () => {} }}
        />,
      ),
    )
  try {
    await render("")
    const form = host.querySelector("form.desktop-composer")
    expect(form?.hasAttribute("data-empty")).toBe(true)
    const wake = host.querySelector<HTMLButtonElement>(".desktop-composer-wake")
    expect(wake?.getAttribute("aria-label")).toBe("Write a message")
    // The circle shows the model's provider mark, not the fallback.
    expect(wake?.querySelector(".desktop-provider-mark")).not.toBeNull()
    // A press puts focus on the circle: the pane takes it from there.
    await act(async () => wake?.click())
    expect(document.activeElement).toBe(wake)
    // Spaces are typed: they keep the line.
    await render("  ")
    expect(form?.hasAttribute("data-empty")).toBe(false)
    await render("draft")
    expect(form?.hasAttribute("data-empty")).toBe(false)
    await render("", [
      {
        id: "a",
        kind: "file",
        name: "notes.txt",
        mimeType: "text/plain",
        size: 1,
        url: "blob:notes",
      },
    ])
    expect(form?.hasAttribute("data-empty")).toBe(false)
  } finally {
    await act(async () => root.unmount())
  }
})

it("says nothing of its own: every note is its control's tooltip, and the form holds only the draft and its row", async () => {
  const root = createRoot(host)
  const warning = (text: string) => ({ tone: "warning" as const, text })
  await act(async () =>
    root.render(
      <Composer
        page={false}
        onPageChange={() => {}}
        onSend={() => {}}
        text="a draft"
        onTextChange={() => {}}
        model={undefined}
        onModelChange={() => {}}
        modelNote={warning("Model didn’t change. Choose again to try.")}
        access={{
          value: "ask-approval",
          onChange: () => {},
          note: warning("This model doesn’t offer that access."),
        }}
        images={{
          images: [],
          onPaste: () => {},
          onRemove: () => {},
          note: warning("An image can be 5 MiB at most."),
        }}
      />,
    ),
  )
  const form = host.querySelector(".desktop-composer")
  if (!form) throw new Error("no composer")
  // Whatever a control has to say, the form itself has three children: the
  // draft, its row of controls, and the circle a quiet pane folds it to.
  expect([...form.children].map((child) => child.className)).toEqual([
    "desktop-composer-body",
    "desktop-composer-row",
    "desktop-composer-wake",
  ])
  expect(form.querySelector('[role="status"], [role="alert"], [aria-live]')).toBeNull()
  const said = (selector: string) => {
    const control = form.querySelector(selector)
    return [
      control?.getAttribute("data-tooltip"),
      control?.getAttribute("data-attention"),
    ]
  }
  expect(said('button[aria-label="Attach files"]')).toEqual([
    "An image can be 5 MiB at most.",
    "warning",
  ])
  expect(said("[data-access-mode]")).toEqual([
    "This model doesn’t offer that access.",
    "warning",
  ])
  // Folded, the controls are hidden: the circle says the first warning.
  expect(said(".desktop-composer-wake")).toEqual([
    "Model didn’t change. Choose again to try.",
    "warning",
  ])
  const picker = form.querySelector(".desktop-chip-model")?.closest(".desktop-tip-anchor")
  expect([
    picker?.getAttribute("data-tooltip"),
    picker?.getAttribute("data-attention"),
  ]).toEqual(["Model didn’t change. Choose again to try.", "warning"])
  await act(async () => root.unmount())
})

it("keeps the model under + in Simple, where + opens the same picker", async () => {
  localStorage.removeItem(appModeKey)
  Object.assign(globalThis, {
    ResizeObserver: class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  })
  const picked: string[] = []
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Composer
        page={false}
        onPageChange={() => {}}
        text=""
        onTextChange={() => {}}
        model={{ provider: "anthropic", modelId: "claude-opus-5" }}
        onModelChange={(model) => picked.push(model.modelId)}
        images={{ images: [], onPaste: () => {}, onRemove: () => {} }}
      />,
    ),
  )
  try {
    const row = host.querySelector(".desktop-composer-row")
    // Simple: + alone at the row's start, the model and Fast not shown there.
    expect(row?.querySelector(".desktop-composer-attach")).toBeNull()
    expect(row?.querySelector(".desktop-composer-fast")).toBeNull()
    const chip = row?.querySelector<HTMLElement>(".desktop-chip-model")
    expect(chip?.closest("[hidden]")).not.toBeNull()
    const more = row?.querySelector<HTMLButtonElement>(".desktop-composer-more")
    expect(more?.getAttribute("aria-label")).toBe("More")
    await act(async () => {
      more?.focus()
      more?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
    })
    const items = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')]
    expect(items.map((item) => item.textContent)).toEqual([
      "Attach files",
      expect.stringMatching(/^Model.+/),
    ])
    await act(async () => items[1]?.click())
    expect(document.querySelector('[data-slot="model-level-picker"]')).not.toBeNull()

    // Advanced mode, from Settings in this window: the row is the full row again.
    await act(async () => {
      localStorage.setItem(appModeKey, advanced)
      window.dispatchEvent(new StorageEvent("storage", { key: appModeKey }))
    })
    expect(host.querySelector(".desktop-composer-more")).toBeNull()
    expect(host.querySelector(".desktop-composer-attach")).not.toBeNull()
    expect(host.querySelector(".desktop-chip-model")?.closest("[hidden]")).toBeNull()
  } finally {
    await act(async () => root.unmount())
  }
})
