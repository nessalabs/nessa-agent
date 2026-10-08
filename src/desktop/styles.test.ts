/**
 * What the stylesheet promises about motion, read from the stylesheet: the
 * glide a first message's composer lands on never passes its place.
 */
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { expect, it } from "vitest"
import { classes, marks } from "./split-panes"

const styles = readFileSync(new URL("./styles.css", import.meta.url), "utf8")

/**
 * A pane in the window's corner — or where a drag's preview would put it
 * there (`marks.dragCorner`, `split-panes/adapters/dom/drag.ts`), and not
 * where it would leave — as the stylesheets select it.
 */
const cornered = `.workspace[data-panes-alone]
  .workspace-pane:is(
    [${marks.corner}]:not([${marks.dragCorner}="no"]),
    [${marks.dragCorner}="yes"]
  )
`

it("glides without overshoot: the composer lands, and never passes its place", () => {
  const curve = /--desktop-glide:\s*linear\(([^)]*)\)/.exec(styles)?.[1]
  expect(curve).toBeDefined()
  const points = (curve ?? "").split(",").map((value) => Number.parseFloat(value))
  expect(points.at(0)).toBe(0)
  expect(points.at(-1)).toBe(1)
  // At most a pixel past its place over a travel of 600px.
  expect(Math.max(...points)).toBeLessThanOrEqual(1 + 1 / 600)
  expect(points.every((point, index) => index === 0 || point >= points[index - 1])).toBe(
    true,
  )
})

it("keeps the surfaces' element resets weightless, so no component's own colour loses to them", () => {
  // `.workspace button { color: inherit }` once outranked `.desktop-send`'s own colour and
  // drew the send arrow in the colour of its fill. A reset under a surface is `:where(...)`.
  const sheets = [
    "./styles.css",
    "./workspace/ui/chrome/chrome.css",
    "./settings/ui/settings.css",
  ].map((path) => readFileSync(new URL(path, import.meta.url), "utf8"))
  const weighty =
    /^[ \t]*\.(?:workspace|settings)\s+(?:button|input|textarea|a|:focus-visible)\b[^{]*\{/gm
  expect(sheets.flatMap((sheet) => sheet.match(weighty) ?? [])).toEqual([])
})

it("starts titlebar-row content at the one safe area, never at a length of its own", () => {
  // The safe area is defined once, from the host's inset and the controls' cluster.
  expect(styles).toMatch(
    /--desktop-titlebar-safe-start:\s*calc\(\s*var\(--desktop-window-controls-inset\)/,
  )
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8")
  const rules = [
    // The titlebar's cluster starts after the host's own controls.
    [
      read("./workspace/ui/chrome/chrome.css"),
      ".workspace-titlebar {",
      "var(--desktop-window-controls-inset)",
    ],
    [
      read("./settings/ui/settings.css"),
      ".settings-titlebar {",
      "var(--desktop-window-controls-inset)",
    ],
    // A pane in the window's corner starts its header at the safe area.
    [
      read("./workspace/ui/panes/panes.css"),
      `${cornered}  .workspace-pane-header {`,
      "var(--desktop-titlebar-safe-start)",
    ],
  ] as const
  for (const [sheet, selector, start] of rules) {
    const body = sheet.slice(sheet.indexOf(selector)).split("}")[0]
    expect(body, selector).toContain(`padding-left: ${start}`)
  }
})

it("starts an inline column title after the window's controls wherever they stand over its column", () => {
  // With the sidebar away, the session list and Settings' page begin at the
  // window's edge, under the controls: an inline title (`ui/column-header.tsx`)
  // starts at the safe area there, never at a length of its own.
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8")
  const rules = [
    [
      read("./workspace/ui/session-list/session-list.css"),
      '.workspace[data-sidebar="closed"] .workspace-list .desktop-column-bar {',
    ],
    [
      read("./settings/ui/settings.css"),
      '.settings[data-sidebar="closed"] .settings-content > .desktop-column-bar {',
    ],
    // The sidebar's own action, in the row the controls stand over.
    [
      read("./workspace/ui/source-list/source-list.css"),
      ".workspace-sidebar > .desktop-column-bar {",
    ],
  ] as const
  for (const [sheet, selector] of rules) {
    expect(sheet, selector).toContain(selector)
    const body = sheet.slice(sheet.indexOf(selector)).split("}")[0]
    expect(body, selector).toMatch(
      /padding-left:[^;]*var\(--desktop-titlebar-safe-start\)/,
    )
  }
})

it("keeps the list's inline title out from under the window's controls while a drop would fold the sidebar", () => {
  // The fold's preview slides the list to the window's edge, under the
  // controls; the title the fold moves to its own row fades from this one.
  const sheet = readFileSync(
    new URL("./workspace/ui/session-list/session-list.css", import.meta.url),
    "utf8",
  )
  // The attribute is the drag's own (`marks.takesSpare`), never retyped.
  const selector = `.workspace[${marks.takesSpare}][data-sidebar="open"]`
  expect(sheet).toContain(selector)
  const body = sheet.slice(sheet.indexOf(selector)).split("}")[0]
  expect(body).toContain(".desktop-column-title")
  expect(body).toMatch(/opacity:\s*0;/)
})

it("spells every split-panes name a host uses as split-panes publishes it", () => {
  // A host's stylesheet cannot import the names (`marks`, `classes`), and a
  // host's code or a verification script that retypes one keeps working
  // until the module renames it — then matches nothing, silently. Every name
  // the module puts on the page begins with one of these, so any a host
  // spells from those families must be a published one.
  const published = new Set<string>([...Object.values(marks), ...Object.values(classes)])
  // A name ends where a name ends; only a file's own name, followed by its
  // extension (`split-panes-drag.test.tsx`), is not one.
  const family =
    /(?<![/\w-])(?:data-(?:split|drag)|split-panes)-[a-z][a-z-]*(?![\w-]|\.(?:test\.)?(?:css|tsx?|mjs)\b)/g
  // And a name read through `dataset` (`dataset.dragCarrying` is `data-drag-carrying`).
  const viaDataset = /\bdataset\.((?:split|drag)[A-Z]\w*)/g
  const kebab = (camel: string) =>
    `data-${camel.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`
  const desktop = fileURLToPath(new URL(".", import.meta.url))
  const root = join(desktop, "../..")
  const walk = (dir: string): string[] =>
    readdirSync(dir).flatMap((name) => {
      const path = join(dir, name)
      if (statSync(path).isDirectory()) return name === "split-panes" ? [] : walk(path)
      return /\.(css|tsx?|mjs)$/.test(name) ? [path] : []
    })
  const files = [...walk(desktop), ...walk(join(root, "verification/desktop/scripts"))]
  const unpublished = files.flatMap((path) =>
    [
      ...[...readFileSync(path, "utf8").matchAll(family)].map((match) => match[0]),
      ...[...readFileSync(path, "utf8").matchAll(viaDataset)].map((match) =>
        kebab(match[1]),
      ),
    ]
      .filter((name) => !published.has(name))
      .map((name) => `${relative(root, path)}: ${name}`),
  )
  expect(unpublished).toEqual([])
  // And the families are what the module sets: none of its names is outside them.
  for (const name of published)
    expect(name).toMatch(/^(?:data-(?:split|drag)|split-panes)-/)
})

it("previews a fold on the attribute the drag sets, in every sheet that previews one", () => {
  // A rule on any other name matches nothing: the preview silently stops.
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8")
  for (const path of [
    "./workspace/ui/chrome/chrome.css",
    "./workspace/ui/session-list/session-list.css",
  ]) {
    const sheet = read(path)
    const named = [...sheet.matchAll(/\[(data-drag-[a-z-]+)\]\[data-sidebar/g)].map(
      (match) => match[1],
    )
    expect(named.length, path).toBeGreaterThan(0)
    for (const name of named) expect(name, path).toBe(marks.takesSpare)
  }
})

it("names what a flight restyles, so beginning one restyles a few elements, not every one in every pane", () => {
  // `.workspace[data-split-flipping] .workspace-pane > *` made the browser restyle all
  // ~1,100 elements of four panes as a flight began: a rule whose subject is any
  // element cannot be aimed. Named, it restyles two per pane.
  const sheet = ["./workspace/ui/panes/panes.css", "./split-panes/ui/split-panes.css"]
    .map((path) => readFileSync(new URL(path, import.meta.url), "utf8"))
    .join("\n")
  // The marks a flight and a preview set, as split-panes publishes them.
  const marked = (name: string) =>
    sheet.match(new RegExp(String.raw`^[^{}/]*\[${name}\][^{]*\{`, "gm")) ?? []
  for (const name of [marks.flipping, marks.reflow])
    expect(marked(name).length, name).toBeGreaterThan(0)
  const flightRules = [...marked(marks.flipping), ...marked(marks.reflow)]
  for (const rule of flightRules) expect(rule, rule).not.toMatch(/(?:>|\s)\*\s*(?:,|\{)/)
})

it("moves a drop's placeholder by transform, not by laying out its size", () => {
  // A width or height transition lays the page out on every frame of a drag.
  const sheet = readFileSync(
    new URL("./split-panes/ui/split-panes.css", import.meta.url),
    "utf8",
  )
  const body = sheet.slice(sheet.indexOf(".split-panes-placeholder {")).split("}")[0]
  const transition = body.match(/transition:[^;]*/)?.[0] ?? ""
  expect(transition).toMatch(/^transition:\s*transform\b/)
  expect(transition).not.toMatch(/\b(?:width|height)\b/)
  // Opaque, and still. A tint over transparent, or a fade, composites the blur
  // on every frame of the move.
  const background = body.match(/background:[^;]*/)?.[0] ?? ""
  expect(background).toMatch(/var\(--background\)/)
  expect(background).not.toMatch(/transparent/)
  expect(body).not.toMatch(/animation:/)
  expect(body).toMatch(/will-change:\s*transform/)
})

it("lifts a drag's copy without painting a shadow", () => {
  // A shadow is painted again each time the copy changes shape.
  const sheet = readFileSync(
    new URL("./split-panes/ui/split-panes.css", import.meta.url),
    "utf8",
  )
  const body = sheet.slice(sheet.indexOf(".split-panes-ghost {")).split("}")[0]
  const shadow = body.match(/box-shadow:[^;]*/)?.[0] ?? ""
  expect(shadow).toMatch(/box-shadow:\s*none/)
  expect(body).toMatch(/opacity:\s*1\b/)
  expect(body).toMatch(/background:\s*var\(--background\)/)
  expect(body).not.toMatch(/will-change/)
  const waiting = sheet
    .slice(sheet.indexOf(".split-panes-ghost[data-drag-waiting] {"))
    .split("}")[0]
  expect(waiting).toMatch(/content-visibility:\s*hidden/)
  expect(waiting).not.toMatch(/will-change/)
  const shown = sheet
    .slice(sheet.indexOf(".split-panes-ghost:not([data-drag-waiting]) {"))
    .split("}")[0]
  expect(shown).toMatch(/will-change:\s*transform/)
  // The copy's box is the size `drag.ts` sets. The stylesheet only keeps
  // that box from laying anything out outside itself.
  const band = sheet.slice(sheet.indexOf(`${classes.ghost} {`)).split("}")[0]
  expect(band).toMatch(/contain:\s*strict/)
  expect(band).not.toMatch(/height:/)
  const picture = sheet.slice(sheet.indexOf(".split-panes-ghost * {")).split("}")[0]
  expect(picture).toMatch(/container-type:\s*normal\s*!important/)
  expect(picture).toMatch(/container-name:\s*none\s*!important/)
  expect(picture).toMatch(/box-shadow:\s*none\s*!important/)
  expect(picture).toMatch(/(?:^|[;\n])\s*filter:\s*none\s*!important/)
  const bodyHidden = sheet
    .slice(sheet.indexOf(".split-panes-ghost .workspace-pane-body {"))
    .split("}")[0]
  expect(bodyHidden).toMatch(/content-visibility:\s*hidden/)
})

it("keeps the ambient blur on its own layer", () => {
  const body = styles.slice(styles.indexOf(".desktop-ambient {")).split("}")[0]
  expect(body).toMatch(/will-change:\s*transform/)
  // The filtered circles are not promoted on their own. The parent layer is.
  const blurAt = styles.indexOf("filter: blur(80px)")
  const blur = styles.slice(
    styles.lastIndexOf(".desktop-ambient::before", blurAt),
    styles.indexOf("}", blurAt),
  )
  expect(blur).not.toMatch(/will-change/)
})

it("holds the ambient blur still while panes travel", () => {
  const selector =
    ":root:is([data-drag-pressing], [data-drag-reflow], [data-split-flipping])\n  .desktop-ambient::before,\n:root:is([data-drag-pressing], [data-drag-reflow], [data-split-flipping])\n  .desktop-ambient::after {"
  expect(styles).toContain(selector)
  const body = styles.slice(styles.indexOf(selector)).split("}")[0]
  expect(body).toMatch(/animation-play-state:\s*paused/)
})

it("drops the composer's blur while a drag is carried and keeps the sidebar treatment stable", () => {
  const selector = ":root[data-drag-pressing] .desktop-composer {"
  expect(styles).toContain(selector)
  const body = styles.slice(styles.indexOf(selector)).split("}")[0]
  expect(body).toMatch(/backdrop-filter:\s*none/)
  expect(body).toMatch(/background:\s*var\(--background\)/)
  // The sidebar's treatment does not change on each gesture.
  expect(styles).not.toContain(":root[data-drag-pressing]\n  :is(.workspace-sidebar")
  expect(styles).not.toContain(
    ".workspace[data-overview-glass]\n  :is(.workspace-sidebar",
  )
  const grain = styles
    .slice(styles.indexOf(":root[data-drag-pressing] .desktop-grain {"))
    .split("}")[0]
  expect(grain).toMatch(/visibility:\s*hidden/)
  // The resting shadow stays rather than changing on release.
  expect(styles).not.toContain(":root[data-drag-pressing] .workspace-sidebar {")
  const rows = readFileSync(
    new URL("./workspace/ui/source-list/source-list.css", import.meta.url),
    "utf8",
  )
  const row = rows
    .slice(rows.indexOf(".workspace[data-overview-glass] .workspace-row {"))
    .split("}")[0]
  expect(row).toMatch(/transition:\s*none/)
  const quiet = rows
    .slice(
      rows.indexOf(
        ".workspace[data-overview-glass] .workspace-row[data-active]:not(.agents-overview-entry)",
      ),
    )
    .split("}")[0]
  expect(quiet).toMatch(/background:\s*transparent/)
  expect(quiet).not.toMatch(/font-weight/)
})

it("lays the grain on as a flat veil, not an overlay blend", () => {
  const body = styles.slice(styles.indexOf(".desktop-grain {")).split("}")[0]
  expect(body).toMatch(/mix-blend-mode:\s*normal/)
  expect(body).not.toMatch(/overlay/)
})

it("does not paint a carried pane's conversation on the frame it lifts", () => {
  const sheet = readFileSync(
    new URL("./workspace/ui/panes/panes.css", import.meta.url),
    "utf8",
  )
  const normalized = sheet.replace(/\s+/g, " ")
  const lifted = normalized
    .slice(
      normalized.indexOf(
        ".workspace:not([data-drag-card]) .workspace-pane[data-drag-lifted] > .workspace-pane-header,",
      ),
    )
    .split("}")[0]
  expect(lifted).toMatch(/content-visibility:\s*hidden/)
  expect(lifted).not.toMatch(/opacity/)
  const slot = sheet
    .slice(
      sheet.indexOf(
        ".workspace:not([data-drag-card]) .workspace-pane[data-drag-lifted] {",
      ),
    )
    .split("}")[0]
  expect(slot).toMatch(/transition:\s*none/)
})

it("skips pane bodies on the frame a drop commits them", () => {
  const sheet = readFileSync(
    new URL("./workspace/ui/panes/panes.css", import.meta.url),
    "utf8",
  )
  const body = sheet
    .slice(sheet.indexOf(".workspace-pane[data-drag-settling] > .workspace-pane-body {"))
    .split("}")[0]
  expect(body).toMatch(/content-visibility:\s*hidden/)
  // While the preview moves, the conversation is not painted. The scroller
  // keeps its box (`drag.mjs`).
  const quiet = sheet
    .slice(sheet.indexOf(".workspace[data-drag-reflow] .workspace-transcript-inner {"))
    .split("}")[0]
  expect(quiet).toMatch(/content-visibility:\s*hidden/)
  const mask = sheet
    .slice(sheet.indexOf(".workspace[data-drag-reflow] .workspace-transcript {"))
    .split("}")[0]
  expect(mask).toMatch(/mask-image:\s*none/)
  const travelling = sheet
    .slice(
      sheet.indexOf(
        ".workspace:is([data-split-flipping], [data-drag-reflow]) .workspace-pane {",
      ),
    )
    .split("}")[0]
  expect(travelling).toMatch(/background:\s*var\(--background\)/)
})

it("keeps the list and the panes under the Agents overview laid out", () => {
  // A pane command asked while the overview is open measures the panes' room:
  // it must be the room they have, so nothing under the overview may leave the
  // layout (display, content-visibility) or change its size. visibility would
  // restyle every descendant as the cover comes and goes; the layer marks
  // them inert instead (`overview-layer.tsx`).
  const sheet = readFileSync(
    new URL("./workspace/ui/layouts/layouts.css", import.meta.url),
    "utf8",
  )
  expect(sheet).not.toMatch(/\[data-overview-covered\][^{]*\{[^}]*visibility/)
  expect(sheet).not.toMatch(/\[data-content="agents"\]/)
  const overview = readFileSync(
    new URL("./workspace/ui/overview/overview.css", import.meta.url),
    "utf8",
  )
  const surface = overview
    .slice(overview.indexOf(".agents-overview-surface {"))
    .split("}")[0]
  expect(surface).not.toMatch(/animation:|0 12px 40px/)
  const bare = overview
    .slice(overview.indexOf(".agents-overview-surface[data-bare] {"))
    .split("}")[0]
  expect(bare).toMatch(/background:\s*none/)
  expect(bare).toMatch(/box-shadow:\s*none/)
  const openLayer = sheet
    .slice(sheet.indexOf(".workspace-overview-layer[data-open] {"))
    .split("}")[0]
  expect(openLayer).not.toMatch(/background/)
  const coveredLayer = sheet
    .slice(sheet.indexOf(".workspace-overview-layer[data-covered] {"))
    .split("}")[0]
  expect(coveredLayer).toMatch(/background:\s*var\(--background\)/)
  expect(coveredLayer).not.toMatch(/desktop-pane-fill/)
  // No rule on what is covered: visibility, display, and content-visibility
  // would restyle or resize it as the cover comes and goes.
  const everyCoveredRule = sheet.match(/\[data-overview-covered\][^{]*\{[^}]*\}/g) ?? []
  expect(everyCoveredRule).toEqual([])
  // The overview's own sheet sets nothing on what is under it.
  expect(overview).not.toMatch(/\.workspace-(?:list|chat)\b/)
})

it("keeps a conversation's sliver of the header still, beneath the text, and out of short panes", () => {
  const body = (selector: string) => styles.slice(styles.indexOf(selector)).split("}")[0]
  expect(styles).toContain(".desktop-header[data-sliver] {")
  // Beneath every word in the pane, and taking no pointer.
  expect(body(".desktop-header[data-sliver] {")).toMatch(/z-index:\s*-1/)
  expect(body(".desktop-header[data-sliver] {")).toMatch(/pointer-events:\s*none/)
  // The night scene's rain and steam hold still in it: no per-frame work.
  expect(styles).toMatch(
    /\.desktop-night-scene\[data-still\] \.desktop-night-scene-rain > span,\s*\.desktop-night-scene\[data-still\] \.desktop-night-scene-steam > span \{\s*animation:\s*none/,
  )
  // Gone from a pane shorter than 220px.
  expect(styles).toMatch(
    /@container workspace-pane \(max-height: 219px\) \{\s*\.desktop-header\[data-sliver\] \{\s*display:\s*none/,
  )
})

it("keeps a running step's time on one line, whatever the pane's width", () => {
  const sheet = readFileSync(
    new URL("./workspace/ui/transcript/transcript.css", import.meta.url),
    "utf8",
  )
  const body = sheet.slice(sheet.indexOf(".workspace-live-time {")).split("}")[0]
  expect(body).toMatch(/white-space:\s*nowrap/)
  expect(body).toMatch(/flex-shrink:\s*0/)
})

it("paints nothing of a pane under the window's controls: its content below the titlebar row, its picture band after them", () => {
  const panes = readFileSync(
    new URL("./workspace/ui/panes/panes.css", import.meta.url),
    "utf8",
  )
  const body = (sheet: string, selector: string) =>
    sheet.slice(sheet.indexOf(selector)).split("}")[0]
  // The header is the titlebar row; what follows it starts below the titlebar's height.
  expect(body(panes, ".workspace-pane-header {")).toMatch(
    /margin-bottom:\s*calc\(\s*var\(--desktop-titlebar-height\) - var\(--desktop-gutter\)/,
  )
  // In the corner, the band begins after the controls.
  const corner = `${cornered}  .desktop-header[data-sliver] {`
  expect(styles).toContain(corner)
  expect(body(styles, corner)).toMatch(
    /left:\s*calc\(var\(--desktop-titlebar-safe-start\) - var\(--desktop-gutter\)\)/,
  )
  // While panes travel, it waits out of sight.
  expect(
    body(
      panes,
      ".workspace:is([data-split-flipping], [data-drag-reflow]) .desktop-header[data-sliver] {",
    ),
  ).toMatch(/opacity:\s*0/)
})

it("moves Settings' sidebar by transform, never by animating its width", () => {
  const sheet = readFileSync(
    new URL("./settings/ui/settings.css", import.meta.url),
    "utf8",
  )
  const body = sheet.slice(sheet.indexOf(".settings-sidebar {")).split("}")[0]
  expect(body).not.toMatch(/transition/)
})

it("keeps pane bodies out of layout during a flight and staged restoration", () => {
  const sheet = readFileSync(
    new URL("./workspace/ui/panes/panes.css", import.meta.url),
    "utf8",
  )
  const rule = sheet
    .slice(
      sheet.indexOf(
        `.workspace[${marks.flipping}] .workspace-pane > .workspace-pane-body,`,
      ),
    )
    .split("}")[0]
  expect(rule).toContain(`.workspace-pane[${marks.restoring}] > .workspace-pane-body`)
  expect(rule).toMatch(/content-visibility:\s*hidden/)
})

it("draws a pill, a short fade and a focus ring from the window's tokens, not from literals", () => {
  // Settings keeps its own sheet until its redesign lands (#632); everything else
  // names the scale in `styles.css` — `--desktop-radius-pill`, `--desktop-fast`,
  // `--desktop-medium`, `--desktop-focus-outline` — so a change to how the app
  // looks is made in one place.
  const root = fileURLToPath(new URL(".", import.meta.url))
  const sheets = (dir: string): string[] =>
    readdirSync(dir).flatMap((name) => {
      const path = join(dir, name)
      if (statSync(path).isDirectory())
        return path === join(root, "settings") ? [] : sheets(path)
      return name.endsWith(".css") ? [path] : []
    })
  const literals = {
    pill: /\b\d*999px\b/,
    fade: /\b(?:120|140|160)ms\b|\b0?\.1[246]s\b/,
    focus:
      /(?<![-\w])outline(?:-width)?:[^;]*\b[\d.]+px\b[^;]*\bsolid\b|(?<![-\w])outline:\s*solid\s+[\d.]+px/,
  }
  const comments = /\/\*[\s\S]*?\*\//g
  // `styles.css` defines the tokens; every other rule uses them.
  const definition = /^\s*--desktop-(?:fast|medium|radius-pill):[^;]*;/gm
  for (const path of sheets(root)) {
    let source = readFileSync(path, "utf8").replace(comments, "")
    if (path === join(root, "styles.css")) source = source.replace(definition, "")
    for (const [name, literal] of Object.entries(literals))
      expect(source, `${relative(root, path)}: ${name}`).not.toMatch(literal)
  }
})

it("defines every token the menus and the pickers read on :root, because they sit outside every surface", () => {
  // A `var()` with no value drops its whole declaration: a menu closed without its
  // fade, the model picker lost its selected row (both caught in review), and the picker's
  // rise never played at the base commit, for the same reason. Menus and the model picker portal to <body>.
  const comments = /\/\*[\s\S]*?\*\//g
  const defined = (source: string) =>
    new Set([...source.matchAll(/(--desktop-[\w-]+)\s*:/g)].map((match) => match[1]))
  const rules = (source: string) =>
    [...source.replace(comments, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({
      selector: match[1],
      body: match[2],
    }))
  const scope = styles.match(/:root,\s*\[data-surface\]\s*\{([^}]*)\}/)?.[1] ?? ""
  const onRoot = defined(scope)
  expect(onRoot.size).toBeGreaterThan(0)
  const menu = readFileSync(new URL("./ui/menu/menu.css", import.meta.url), "utf8")
  const portalled = [
    ...rules(menu),
    ...rules(styles).filter(({ selector }) =>
      /\.desktop-popover|\.desktop-model-picker/.test(selector),
    ),
  ]
  // What the portalled rules define themselves (the popover material) is theirs.
  const own = new Set(portalled.flatMap(({ body }) => [...defined(body)]))
  const read = new Set<string>()
  for (const { body } of portalled)
    for (const [, token] of body.matchAll(/var\((--desktop-[\w-]+)/g))
      // `--desktop-light-*` is set on <html> by the theme (`adapters/theme-preference.ts`).
      if (!own.has(token) && !token.startsWith("--desktop-light-")) read.add(token)
  expect(read.size).toBeGreaterThan(0)
  for (const token of read) expect(onRoot, token).toContain(token)
})

it("draws every key cap with the kit's Kbd: no raw <kbd> outside Settings and onboarding", () => {
  // Settings keeps its own until its redesign (#632); onboarding's keycaps are a
  // larger, separate drawing. Everything else is `Kbd`, skinned once in chrome.css.
  const root = fileURLToPath(new URL(".", import.meta.url))
  const sources = (dir: string): string[] =>
    readdirSync(dir).flatMap((name) => {
      const path = join(dir, name)
      if (statSync(path).isDirectory())
        return path === join(root, "settings") ? [] : sources(path)
      return /\.tsx$/.test(name) && !/\.test\./.test(name) ? [path] : []
    })
  for (const path of sources(root))
    expect(readFileSync(path, "utf8"), relative(root, path)).not.toMatch(/<kbd[\s>]/)
})
