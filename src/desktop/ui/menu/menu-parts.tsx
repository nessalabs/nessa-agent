import { createContext, useContext, type ComponentProps } from "react"
import { createPortal } from "react-dom"
import {
  ContextMenu,
  ContextMenuCheckboxItem,
  ContextMenuContent as NessaContextMenuContent,
  ContextMenuGroup,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuRadioGroup,
  ContextMenuRadioItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@nessa-ui/react/context-menu"
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent as NessaDropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
  type DropdownMenuContentProps,
  type DropdownMenuItemProps,
  type DropdownMenuSubTriggerProps,
} from "@nessa-ui/react/dropdown-menu"
import "./menu.css"

/**
 * Which kind of menu a part is drawn in. Radix keeps a dropdown's parts and
 * a context menu's apart, so every part below asks and renders the matching
 * one; a list of items is then written once and shown in either.
 */
type MenuKind = "dropdown" | "context"

const MenuKindContext = createContext<MenuKind | null>(null)

function useMenuKind(part: string): MenuKind {
  const kind = useContext(MenuKindContext)
  if (kind === null) {
    throw new Error(`${part} belongs inside a DropdownMenuContent or ContextMenuContent`)
  }
  return kind
}

/** The class every menu surface wears; `menu.css` draws what it selects. */
const surface = (className: string | undefined) =>
  className ? `desktop-popover ${className}` : "desktop-popover"

type SelectEvent = Parameters<NonNullable<DropdownMenuItemProps["onSelect"]>>[0]

/**
 * Marks the item chosen so it blinks once while its menu fades, as macOS's
 * do. The caller's own handler runs first and still decides: an item that
 * keeps its menu open (`preventDefault`) is not chosen and does not blink.
 */
function chosen(onSelect: ((event: SelectEvent) => void) | undefined) {
  return (event: SelectEvent) => {
    onSelect?.(event)
    if (event.defaultPrevented) return
    if (event.currentTarget instanceof HTMLElement)
      event.currentTarget.dataset.chosen = ""
  }
}

/** A dropdown's floating surface, in the window's glass. */
export function DropdownMenuContent({ className, ...props }: DropdownMenuContentProps) {
  return (
    <MenuKindContext.Provider value="dropdown">
      <NessaDropdownMenuContent className={surface(className)} {...props} />
    </MenuKindContext.Provider>
  )
}

/** A right-click menu's floating surface, in the window's glass. */
export function ContextMenuContent({
  className,
  ...props
}: ComponentProps<typeof NessaContextMenuContent>) {
  return (
    <MenuKindContext.Provider value="context">
      <NessaContextMenuContent className={surface(className)} {...props} />
    </MenuKindContext.Provider>
  )
}

/** One action. Put a `MenuShortcut` after its words for a key that does the same. */
export function MenuItem({ onSelect, ...props }: DropdownMenuItemProps) {
  const Item = useMenuKind("MenuItem") === "context" ? ContextMenuItem : DropdownMenuItem
  return <Item onSelect={chosen(onSelect)} {...props} />
}

/**
 * An item that is on or off, marked by a check in the menu's leading column.
 * Choosing it closes the menu, in either kind, unless `onSelect` prevents it.
 */
export function MenuCheckboxItem({
  onSelect,
  ...props
}: ComponentProps<typeof DropdownMenuCheckboxItem>) {
  return useMenuKind("MenuCheckboxItem") === "context" ? (
    <ContextMenuCheckboxItem closeOnSelect onSelect={chosen(onSelect)} {...props} />
  ) : (
    <DropdownMenuCheckboxItem onSelect={chosen(onSelect)} {...props} />
  )
}

/** One of a `MenuRadioGroup`'s options; the chosen one is checked. */
export function MenuRadioItem({
  onSelect,
  ...props
}: ComponentProps<typeof DropdownMenuRadioItem>) {
  const Item =
    useMenuKind("MenuRadioItem") === "context"
      ? ContextMenuRadioItem
      : DropdownMenuRadioItem
  return <Item onSelect={chosen(onSelect)} {...props} />
}

/** Options of which exactly one is chosen. */
export function MenuRadioGroup(props: ComponentProps<typeof DropdownMenuRadioGroup>) {
  const Group =
    useMenuKind("MenuRadioGroup") === "context"
      ? ContextMenuRadioGroup
      : DropdownMenuRadioGroup
  return <Group {...props} />
}

/** Items that belong together, usually under a `MenuLabel`. */
export function MenuGroup(props: ComponentProps<typeof DropdownMenuGroup>) {
  const Group =
    useMenuKind("MenuGroup") === "context" ? ContextMenuGroup : DropdownMenuGroup
  return <Group {...props} />
}

/** A section's small, quiet heading. Not an item: nothing chooses it. */
export function MenuLabel(props: ComponentProps<typeof DropdownMenuLabel>) {
  const Label =
    useMenuKind("MenuLabel") === "context" ? ContextMenuLabel : DropdownMenuLabel
  return <Label {...props} />
}

/** A hairline between sections. */
export function MenuSeparator(props: ComponentProps<typeof DropdownMenuSeparator>) {
  const Separator =
    useMenuKind("MenuSeparator") === "context"
      ? ContextMenuSeparator
      : DropdownMenuSeparator
  return <Separator {...props} />
}

/** The keys that do what the item does, quiet at its right edge. */
export function MenuShortcut(props: ComponentProps<"span">) {
  const Shortcut =
    useMenuKind("MenuShortcut") === "context" ? ContextMenuShortcut : DropdownMenuShortcut
  return <Shortcut {...props} />
}

/** A submenu: a `MenuSubTrigger` and the `MenuSubContent` it opens. */
export function MenuSub(props: ComponentProps<typeof DropdownMenuSub>) {
  const Sub = useMenuKind("MenuSub") === "context" ? ContextMenuSub : DropdownMenuSub
  return <Sub {...props} />
}

/**
 * The item that opens a submenu, with its chevron at the right edge. It opens
 * on hover — Radix keeps it open while the pointer crosses toward it — and on
 * the → key.
 */
export function MenuSubTrigger(props: DropdownMenuSubTriggerProps) {
  const Trigger =
    useMenuKind("MenuSubTrigger") === "context"
      ? ContextMenuSubTrigger
      : DropdownMenuSubTrigger
  return <Trigger {...props} />
}

/**
 * A submenu's surface, in the same glass as its menu. nessa_ui's draws it
 * where it is written, inside the menu's own surface — which scrolls, and so
 * clips it: open, and drawn nowhere. It is portalled beside the menu instead,
 * as the menu itself is, onto the page's body.
 */
export function MenuSubContent({
  className,
  ...props
}: ComponentProps<typeof DropdownMenuSubContent>) {
  const Content =
    useMenuKind("MenuSubContent") === "context"
      ? ContextMenuSubContent
      : DropdownMenuSubContent
  return createPortal(
    <Content className={surface(className)} {...props} />,
    document.body,
  )
}

export { ContextMenu, ContextMenuTrigger, DropdownMenu, DropdownMenuTrigger }
