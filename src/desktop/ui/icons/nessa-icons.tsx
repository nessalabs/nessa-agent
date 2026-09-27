import type { ReactNode } from "react"
import type { DesktopIconComponent, DesktopIconFamily } from "./icon-contract"

/**
 * Nessa's own icons, drawn for the window rather than borrowed: a 20×20 grid,
 * one quiet 1.4 stroke with round caps and joins, and room around every
 * shape. Keylines keep them one optical size: a circle about 13 across, a
 * square 13 with corners of 3, a window shape 14×12. They are the built-in
 * default every role falls back to.
 *
 * The family owns its stroke, so it is set on a group inside the SVG: a
 * consumer's `svg { stroke-width }` would otherwise reach the paths by
 * inheritance and redraw them in someone else's weight. Size and colour stay
 * the consumer's — width and height from `size` or CSS, paint in currentColor.
 */
function drawn(art: ReactNode, stroke = 1.4): DesktopIconComponent {
  return function NessaIcon({ size = 20, ...props }) {
    return (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 20 20"
        width={size}
        height={size}
        fill="none"
        {...props}
      >
        <g
          fill="none"
          stroke="currentColor"
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          {art}
        </g>
      </svg>
    )
  }
}

/** A filled dot, for the points in "more", a list, or an exclamation. */
function Dot({ x, y, r = 1 }: { x: number; y: number; r?: number }) {
  return <circle cx={x} cy={y} r={r} fill="currentColor" stroke="none" />
}

const pane = <rect x="3" y="4" width="14" height="12" rx="3" />
const shield = (
  <path d="M10 3.5l5.5 2v4.25c0 3.35-2.3 5.7-5.5 6.75-3.2-1.05-5.5-3.4-5.5-6.75V5.5z" />
)
const folder = (
  <path d="M3.5 6.25c0-.97.78-1.75 1.75-1.75h2.9l1.75 1.75h4.85c.97 0 1.75.78 1.75 1.75v5.75c0 .97-.78 1.75-1.75 1.75h-9.5c-.97 0-1.75-.78-1.75-1.75z" />
)

export const nessaIcons: DesktopIconFamily = {
  check: drawn(<path d="M5.25 10.25l3.25 3.25 6.25-6.5" />),
  close: drawn(<path d="M6 6l8 8M14 6l-8 8" />),
  chevronDown: drawn(<path d="M6.25 8.25 10 12l3.75-3.75" />),
  chevronUp: drawn(<path d="M6.25 11.75 10 8l3.75 3.75" />),
  chevronLeft: drawn(<path d="M11.75 6.25 8 10l3.75 3.75" />),
  chevronRight: drawn(<path d="M8.25 6.25 12 10l-3.75 3.75" />),
  moreHorizontal: drawn(
    <>
      <Dot x={5} y={10} r={1.15} />
      <Dot x={10} y={10} r={1.15} />
      <Dot x={15} y={10} r={1.15} />
    </>,
  ),

  sidebar: drawn(
    <>
      {pane}
      <path d="M8 4v12" />
    </>,
  ),
  panelRight: drawn(
    <>
      {pane}
      <path d="M12 4v12" />
    </>,
  ),
  sessionList: drawn(
    <>
      <path d="M8 6.5h8M8 10h8M8 13.5h5" />
      <Dot x={4.5} y={6.5} r={0.95} />
      <Dot x={4.5} y={10} r={0.95} />
      <Dot x={4.5} y={13.5} r={0.95} />
    </>,
  ),
  splitRight: drawn(
    <>
      {pane}
      <path d="M10 4v12" />
    </>,
  ),
  splitDown: drawn(
    <>
      {pane}
      <path d="M3 10h14" />
    </>,
  ),
  maximize: drawn(
    <path d="M11.75 4.25h4v4M15.75 4.25 11.5 8.5M8.25 15.75h-4v-4M4.25 15.75 8.5 11.5" />,
  ),
  restore: drawn(
    <path d="M8.5 4.5v4h-4M8.5 8.5 4.25 4.25M11.5 15.5v-4h4M11.5 11.5l4.25 4.25" />,
  ),
  back: drawn(<path d="M15.5 10h-11M8.75 5.75 4.5 10l4.25 4.25" />),
  forward: drawn(<path d="M4.5 10h11M11.25 5.75 15.5 10l-4.25 4.25" />),
  home: drawn(
    <>
      <path d="M3.75 9 10 3.75 16.25 9" />
      <path d="M5.5 7.75v6.75c0 .83.67 1.5 1.5 1.5h6c.83 0 1.5-.67 1.5-1.5V7.75" />
      <path d="M8.5 16v-2.75a1.5 1.5 0 0 1 3 0V16" />
    </>,
  ),
  search: drawn(
    <>
      <circle cx="8.75" cy="8.75" r="5" />
      <path d="M12.5 12.5 16 16" />
    </>,
  ),

  newSession: drawn(
    <>
      <path d="M9.25 4H6.75A2.75 2.75 0 0 0 4 6.75v6.5A2.75 2.75 0 0 0 6.75 16h6.5A2.75 2.75 0 0 0 16 13.25v-2.5" />
      <path d="M8.75 11.25l.35-2.1 5.4-5.4a1.24 1.24 0 0 1 1.75 1.75l-5.4 5.4z" />
    </>,
  ),
  add: drawn(<path d="M10 4.5v11M4.5 10h11" />),
  channel: drawn(
    <path d="M8.25 4 6.75 16M13.25 4l-1.5 12M4.5 7.75h11.25M4.25 12.25H15.5" />,
  ),
  privateChannel: drawn(
    <>
      <rect x="4.75" y="8.75" width="10.5" height="7.5" rx="2.25" />
      <path d="M7 8.75V7a3 3 0 0 1 6 0v1.75" />
    </>,
  ),
  needsYou: drawn(
    <>
      {shield}
      <path d="M10 7.25v3.25" />
      <Dot x={10} y={12.9} r={0.95} />
    </>,
  ),
  running: drawn(
    <>
      <path d="M16 10a6 6 0 1 1-6-6" />
      <Dot x={10} y={10} r={1.4} />
    </>,
  ),

  // On send's filled disc a slightly firmer line keeps the arrow from thinning out.
  send: drawn(<path d="M10 15.5v-11M5.5 9 10 4.5 14.5 9" />, 1.6),
  attach: drawn(
    <path d="M14.5 9.25l-4.9 4.9a3.2 3.2 0 0 1-4.5-4.5l5.4-5.4a2.1 2.1 0 0 1 3 3l-5.3 5.3a1.05 1.05 0 0 1-1.5-1.5l4.8-4.8" />,
  ),
  folder: drawn(folder),
  folderAdd: drawn(
    <>
      {folder}
      <path d="M10 8.75v4M8 10.75h4" />
    </>,
  ),
  thinking: drawn(
    <path d="M10 3.5c.45 3.55 2.95 6.05 6.5 6.5-3.55.45-6.05 2.95-6.5 6.5-.45-3.55-2.95-6.05-6.5-6.5 3.55-.45 6.05-2.95 6.5-6.5z" />,
  ),
  fast: drawn(<path d="M11.25 3.5 5 11.25h4.75L8.75 16.5 15 8.75h-4.75z" />),
  access: drawn(shield),
  enter: drawn(
    <path d="M15.5 4.5v4.75a2.25 2.25 0 0 1-2.25 2.25H4.75M8 8.25 4.75 11.5 8 14.75" />,
  ),

  customize: drawn(
    <>
      <path d="M15.75 4.25 10.1 9.9" />
      <path d="M8.9 10.4c-1.55-.3-3 .7-3.25 2.3-.15 1-.6 1.75-1.4 2.3 2.1.7 4.65.35 5.55-1.35.55-1.05.35-2.35-.9-3.25z" />
    </>,
  ),
  nightScene: drawn(
    <path d="M15.75 11.75A6.25 6.25 0 1 1 8.25 4.25a5 5 0 0 0 7.5 7.5z" />,
  ),
  chooseImage: drawn(
    <>
      <path d="M16 10.5v2.75A2.75 2.75 0 0 1 13.25 16h-6.5A2.75 2.75 0 0 1 4 13.25v-6.5A2.75 2.75 0 0 1 6.75 4H9.5" />
      <path d="M4.5 13.75 7.75 10.5 10 12.75l1.5-1.5 3.75 3.75" />
      <path d="M14 3.5v4M12 5.5h4" />
    </>,
  ),
  adjustImage: drawn(
    <path d="M10 3.5v13M3.5 10h13M8 5.5l2-2 2 2M8 14.5l2 2 2-2M5.5 8l-2 2 2 2M14.5 8l2 2-2 2" />,
  ),
  zoomIn: drawn(<path d="M5.5 10h9M10 5.5v9" />),
  zoomOut: drawn(<path d="M5.5 10h9" />),

  file: drawn(
    <>
      <path d="M11.25 3.75H7a2 2 0 0 0-2 2v8.5a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2V7.5z" />
      <path d="M11.25 3.75V6.5a1 1 0 0 0 1 1H15" />
      <path d="M7.75 11h4.5M7.75 13.5h3" />
    </>,
  ),
  edit: drawn(
    <>
      <path d="M4.5 15.5l.55-2.75 8.2-8.2a1.56 1.56 0 0 1 2.2 2.2l-8.2 8.2z" />
      <path d="M11.75 6.25l2 2" />
    </>,
  ),
  terminal: drawn(
    <>
      {pane}
      <path d="M6.5 8l2 2-2 2M10.5 12.25h3" />
    </>,
  ),

  preferences: drawn(
    <>
      <path d="M4 7h5.5M13.5 7H16M4 13h2.5M10.5 13H16" />
      <circle cx="11.5" cy="7" r="2" />
      <circle cx="8.5" cy="13" r="2" />
    </>,
  ),
  appearance: drawn(
    <>
      <circle cx="10" cy="10" r="6.25" />
      <path d="M10 3.75a6.25 6.25 0 0 1 0 12.5z" fill="currentColor" stroke="none" />
    </>,
  ),
  workspace: drawn(
    <>
      <rect x="3.75" y="3.75" width="5.25" height="5.25" rx="1.5" />
      <rect x="11" y="3.75" width="5.25" height="5.25" rx="1.5" />
      <rect x="3.75" y="11" width="5.25" height="5.25" rx="1.5" />
      <rect x="11" y="11" width="5.25" height="5.25" rx="1.5" />
    </>,
  ),
  model: drawn(
    <path d="M10 3.5l5.75 3.25v6.5L10 16.5l-5.75-3.25v-6.5zM4.5 6.9 10 10l5.5-3.1M10 10v6.25" />,
  ),
  connections: drawn(
    <>
      <path d="M8 12l4-4" />
      <circle cx="6.25" cy="13.75" r="2.25" />
      <circle cx="13.75" cy="6.25" r="2.25" />
    </>,
  ),
  privacy: drawn(
    <>
      {shield}
      <path d="M7.75 10.1l1.6 1.6 3-3.2" />
    </>,
  ),
  about: drawn(
    <>
      <circle cx="10" cy="10" r="6.5" />
      <path d="M10 9.25v4.25" />
      <Dot x={10} y={6.9} r={0.95} />
    </>,
  ),
}
