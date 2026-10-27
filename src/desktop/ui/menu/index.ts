/**
 * The window's menus: nessa_ui's dropdown and context menus, whose rows the
 * kit's menu recipe draws — the item's icon leading, a check and any
 * shortcut trailing — on a solid card with the theme's light for the
 * highlight, and a chosen item that blinks once as the menu fades.
 *
 * ```text
 * callers ──▶ menu-parts ──▶ @nessa-ui/react/dropdown-menu
 *                 │      └──▶ @nessa-ui/react/context-menu
 *                 ▼
 *             menu.css ──▶ the popover surface in styles.css (.desktop-popover)
 * ```
 *
 * An arrow points from a module to one it depends on. A menu is a
 * `DropdownMenu` or `ContextMenu` root with its trigger and content; the
 * content says which kind it is, so the `Menu*` parts inside — items, labels,
 * separators, submenus — are written once and render in either.
 *
 * The look lives in `menu.css` and selects any Radix menu that wears
 * `.desktop-popover`, so a menu still built from nessa_ui's parts directly
 * looks the same; these parts add the kind switching and the chosen blink.
 */
export {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuCheckboxItem,
  MenuGroup,
  MenuItem,
  MenuLabel,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
  MenuShortcut,
  MenuSub,
  MenuSubContent,
  MenuSubTrigger,
} from "./menu-parts"
