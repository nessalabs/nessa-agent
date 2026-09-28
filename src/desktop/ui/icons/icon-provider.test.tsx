/**
 * The contract's resolution order through real providers: component prop,
 * nearest provider, parent provider, built-in family. Rendered to markup, so
 * what is checked is what a person would get drawn.
 */
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"
import {
  desktopIconRoles,
  type DesktopIconComponent,
  type DesktopIconRole,
} from "./icon-contract"
import { DesktopIcon, DesktopIconProvider, useDesktopIcon } from "./icon-provider"
import { lucideIcons } from "./lucide-icons"
import { nessaIcons } from "./nessa-icons"

const drawing = (name: string): DesktopIconComponent => {
  const icon: DesktopIconComponent = (props) => (
    <svg data-drawing={name} className={props.className} />
  )
  return icon
}

const drawn = (tree: React.ReactNode) => {
  const markup = renderToStaticMarkup(tree)
  return [...markup.matchAll(/data-drawing="([^"]+)"/g)].map((match) => match[1])
}

function Resolved({ role }: { role: DesktopIconRole }) {
  const Icon = useDesktopIcon(role)
  return <Icon />
}

describe("icon resolution through providers", () => {
  const tree = (icon?: DesktopIconComponent) => (
    <DesktopIconProvider icons={{ close: drawing("parent"), search: drawing("parent") }}>
      <DesktopIconProvider icons={{ close: drawing("nearest") }}>
        <DesktopIcon name="close" icon={icon} />
        <DesktopIcon name="search" />
      </DesktopIconProvider>
    </DesktopIconProvider>
  )

  it("draws the component's own icon first", () => {
    expect(drawn(tree(drawing("local")))[0]).toBe("local")
  })

  it("then the nearest provider's", () => {
    expect(drawn(tree())[0]).toBe("nearest")
  })

  it("then a parent provider's, for roles the nearest does not draw", () => {
    expect(drawn(tree())[1]).toBe("parent")
  })

  it("then the built-in family, with no provider at all", () => {
    const markup = renderToStaticMarkup(<Resolved role="close" />)
    expect(markup).toBe(renderToStaticMarkup(nessaIcons.close({})))
  })

  it("draws a whole third-party family through the same provider", () => {
    const markup = renderToStaticMarkup(
      <DesktopIconProvider icons={lucideIcons}>
        <Resolved role="sidebar" />
      </DesktopIconProvider>,
    )
    expect(markup).toContain("lucide-panel-left")
  })
})

describe("DesktopIcon", () => {
  it("is decorative unless told otherwise", () => {
    const markup = renderToStaticMarkup(<DesktopIcon name="search" />)
    expect(markup).toContain('aria-hidden="true"')
    expect(markup).toContain('focusable="false"')
  })

  it("leaves size, colour and class to the consumer", () => {
    const markup = renderToStaticMarkup(
      <DesktopIcon name="search" size={14} className="quiet" aria-hidden={false} />,
    )
    expect(markup).toContain('width="14"')
    expect(markup).toContain('class="quiet"')
    expect(markup).toContain('aria-hidden="false"')
    expect(markup).toContain('stroke="currentColor"')
  })
})

describe("both families", () => {
  it("draw every role, each as a drawing of its own", () => {
    for (const [name, family] of [
      ["nessa", nessaIcons],
      ["lucide", lucideIcons],
    ] as const)
      for (const role of desktopIconRoles)
        expect(renderToStaticMarkup(family[role]({})), `${name} ${role}`).toMatch(
          /^<svg[^>]*><\w/,
        )
  })

  it("draw Settings' Advanced as a flask", () => {
    expect(renderToStaticMarkup(lucideIcons.advanced({}))).toContain(
      "lucide-flask-conical",
    )
    const flask = renderToStaticMarkup(nessaIcons.advanced({}))
    expect(flask).toMatch(/<svg[^>]*viewBox="0 0 20 20"/)
    expect(flask).toMatch(/<g[^>]*stroke-width="1.4"/)
    expect(flask).not.toBe(renderToStaticMarkup(nessaIcons.about({})))
  })
})

describe("the Nessa family", () => {
  it("owns its stroke inside the SVG, out of reach of a consumer's svg rule", () => {
    const markup = renderToStaticMarkup(nessaIcons.search({}))
    expect(markup).toMatch(/<svg[^>]*viewBox="0 0 20 20"/)
    expect(markup).toMatch(/<g[^>]*stroke-width="1.4"/)
  })
})
