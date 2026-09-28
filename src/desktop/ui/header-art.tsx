import { memo, useEffect, useRef, useState } from "react"
import {
  useHeaderFraming,
  useHeaderImage,
  useHeaderPictureUrl,
  useTintFromPicture,
} from "../adapters/header-image"
import { readImagePixels } from "../adapters/image-pixels"
import {
  checkHeaderImage,
  defaultHeaderFraming,
  headerImageRefusalText,
  type HeaderFraming,
} from "../model/header-image"
import {
  extractPalette,
  paletteCss,
  themeFromPalette,
  type PaletteColor,
} from "../model/image-palette"
import { HeaderPicture } from "./header-picture"
import { DesktopIcon } from "./icons"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuCheckboxItem,
  MenuItem,
  MenuLabel,
  MenuSeparator,
} from "./menu"
import { NightScene } from "./night-scene"

/** How long a refused file's reason stays on screen. */
const refusalMs = 4000

/**
 * The home header: the night scene, or a picture the person chose, GIFs
 * included, blended into the window the same way the scene is. A Customize
 * control in its corner switches between them and opens the picture's
 * framing (`ui/header-picture.tsx`), which a new picture starts in.
 *
 * A picture also lends the window its colours: its palette
 * (`model/image-palette.ts`) is shown in the menu, and the theme it implies
 * tints the whole window unless the person turns that off.
 */
export function HeaderArt() {
  const [image, choose, clear] = useHeaderImage()
  const [framing, keepFraming] = useHeaderFraming()
  // While adjusting, the framing being tried; it replaces the kept one on Done.
  const [draft, setDraft] = useState<HeaderFraming | null>(null)
  const adjusting = draft !== null
  const adjustingRef = useRef(adjusting)
  adjustingRef.current = adjusting
  const url = useHeaderPictureUrl()
  const [refusal, setRefusal] = useState<string | null>(null)
  const input = useRef<HTMLInputElement>(null)
  const headerRef = useRef<HTMLDivElement>(null)
  const [palette, setPalette] = useState<PaletteColor[]>([])
  const [tint, setTint] = useTintFromPicture()

  // The picture's main colours. A picture the webview cannot decode simply
  // has none, and the window keeps its own theme.
  useEffect(() => {
    setPalette([])
    if (!image) return
    let current = true
    readImagePixels(image)
      .then((pixels) => {
        if (current) setPalette(extractPalette(pixels))
      })
      .catch(() => {})
    return () => {
      current = false
    }
  }, [image])

  // Tinting the window: the picture's theme is written over the chosen one on
  // the surface that carries it, and taken off again, restoring that theme,
  // when the picture goes or tinting is turned off.
  useEffect(() => {
    const surface = headerRef.current?.closest<HTMLElement>("[data-desktop-theme]")
    if (!surface || !tint || palette.length === 0) return
    const theme = themeFromPalette(palette)
    surface.style.setProperty("--desktop-light-high", theme.high)
    surface.style.setProperty("--desktop-light-low", theme.low)
    surface.style.setProperty("--desktop-light-edge", theme.edge)
    surface.dataset.tintedFromPicture = ""
    return () => {
      surface.style.removeProperty("--desktop-light-high")
      surface.style.removeProperty("--desktop-light-low")
      surface.style.removeProperty("--desktop-light-edge")
      delete surface.dataset.tintedFromPicture
    }
  }, [palette, tint])

  useEffect(() => {
    if (!refusal) return
    const timer = window.setTimeout(() => setRefusal(null), refusalMs)
    return () => window.clearTimeout(timer)
  }, [refusal])

  const accept = (file: File | undefined) => {
    if (!file) return
    const check = checkHeaderImage(file)
    if (!check.ok) {
      setRefusal(headerImageRefusalText[check.reason])
      return
    }
    setRefusal(null)
    choose(file)
    // A new picture starts centred, and straight into adjusting it.
    keepFraming(defaultHeaderFraming)
    setDraft(defaultHeaderFraming)
  }

  return (
    <div
      ref={headerRef}
      className="desktop-header"
      data-adjusting={adjusting ? "" : undefined}
    >
      {url ? (
        <HeaderPicture
          url={url}
          framing={draft ?? framing}
          adjusting={adjusting}
          onFramingChange={setDraft}
          onDone={() => {
            if (draft) keepFraming(draft)
            setDraft(null)
          }}
          onCancel={() => setDraft(null)}
        />
      ) : (
        <NightScene />
      )}
      <div className="desktop-header-controls">
        {refusal ? (
          <span className="desktop-header-refusal" role="status">
            {refusal}
          </span>
        ) : null}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button type="button" className="desktop-chip desktop-header-customize">
              <DesktopIcon name="customize" />
              <span>Customize</span>
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            side="bottom"
            align="end"
            sideOffset={8}
            // Adjusting takes focus to the picture; the menu must not take it back.
            onCloseAutoFocus={(event) => {
              if (adjustingRef.current) event.preventDefault()
            }}
          >
            <MenuLabel>Header</MenuLabel>
            {/* Choosing the scene again forgets the picture. */}
            <MenuItem
              onSelect={() => {
                setDraft(null)
                clear()
              }}
              disabled={!image}
            >
              Night scene
            </MenuItem>
            <MenuItem onSelect={() => input.current?.click()}>
              Choose image or GIF…
            </MenuItem>
            {image ? (
              <MenuItem onSelect={() => setDraft(framing)}>Adjust position…</MenuItem>
            ) : null}
            {image && palette.length > 0 ? (
              <>
                <MenuSeparator />
                <MenuLabel>Colors from your picture</MenuLabel>
                <div className="desktop-palette" aria-hidden="true">
                  {palette.map((entry) => (
                    <span
                      key={paletteCss(entry.color)}
                      className="desktop-palette-swatch"
                      style={{
                        background: paletteCss(entry.color),
                        flexGrow: 0.4 + entry.weight,
                      }}
                    />
                  ))}
                </div>
                <MenuCheckboxItem
                  checked={tint}
                  onCheckedChange={(checked) => setTint(checked === true)}
                  onSelect={(event) => event.preventDefault()}
                >
                  Tint app from picture
                </MenuCheckboxItem>
              </>
            ) : null}
          </DropdownMenuContent>
        </DropdownMenu>
        <input
          ref={input}
          type="file"
          accept="image/*"
          hidden
          onChange={(event) => {
            accept(event.target.files?.[0])
            event.target.value = ""
          }}
        />
      </div>
    </div>
  )
}

const keepAsIs = () => {}

/**
 * A sliver of the same header at the top of a conversation pane: the picture
 * in the framing the home header keeps, or the night scene when none is
 * chosen, drawn by the same parts (`HeaderPicture`, `NightScene`) and
 * narrowed by the stylesheet (`.desktop-header[data-sliver]`) to a low band
 * that fades into the pane. It never moves on its own: the night scene's rain
 * and steam are still here, and a GIF moves only in the `moving` — focused —
 * pane; the others show its first frame. Decoration only.
 */
export const HeaderSliver = memo(function HeaderSliver({ moving }: { moving: boolean }) {
  const [image] = useHeaderImage()
  const [framing] = useHeaderFraming()
  const url = useHeaderPictureUrl(!moving)
  return (
    <div className="desktop-header" data-sliver="" aria-hidden="true">
      {image ? (
        url ? (
          <HeaderPicture
            url={url}
            framing={framing}
            adjusting={false}
            onFramingChange={keepAsIs}
            onDone={keepAsIs}
            onCancel={keepAsIs}
          />
        ) : null
      ) : (
        <NightScene still />
      )}
    </div>
  )
})
