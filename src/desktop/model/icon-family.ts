/**
 * The icon families the window can draw its chrome with. A family owns only
 * the artwork of each semantic role; `ui/icons/` holds the drawings and the
 * provider that resolves roles through the chosen family.
 */
export const iconFamilies = [
  { id: "nessa", label: "Nessa" },
  { id: "lucide", label: "Lucide" },
] as const

export type IconFamilyId = (typeof iconFamilies)[number]["id"]

export const defaultIconFamily: IconFamilyId = "nessa"

/** Reads a stored or requested family, falling back to the default for anything unknown. */
export function parseIconFamily(value: unknown): IconFamilyId {
  return iconFamilies.find((family) => family.id === value)?.id ?? defaultIconFamily
}
