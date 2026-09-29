/**
 * Where a column's title sits: inline in the titlebar row — after the
 * window's controls where they stand over the column, before the column's
 * own action — when it fits there with room to breathe, and on a row of its
 * own below the titlebar when it does not. Shared by the workspace's session
 * list and Settings' page (`ui/column-header.tsx`), which measure and ask.
 *
 * | placement now | the row's room for the title                   | next    |
 * | ------------- | ---------------------------------------------- | ------- |
 * | none yet      | title + breathing room fits                    | inline  |
 * | none yet      | it does not                                    | below   |
 * | below         | title + breathing room fits                    | inline  |
 * | inline        | still fits with `hold` px to spare taken back   | inline  |
 * | inline        | short by more than `hold`                      | below   |
 *
 * The hold keeps a column dragged near the threshold from flipping its title
 * back and forth with every pixel.
 */

export type TitlePlacement = "inline" | "below"

/** Clear space kept between an inline title and what follows it in the row, in px. */
export const titleBreathingRoom = 24

/** How far an inline title may eat into its breathing room before it goes below, in px. */
export const titlePlacementHold = 12

export function titlePlacement({
  room,
  title,
  now,
}: {
  /** The titlebar row's width free for the title: after the controls, before the column's action. */
  readonly room: number
  /** The title's own width, set inline, unwrapped. */
  readonly title: number
  readonly now: TitlePlacement | null
}): TitlePlacement {
  const needed = title + titleBreathingRoom - (now === "inline" ? titlePlacementHold : 0)
  return room >= needed ? "inline" : "below"
}
