/**
 * When the home composer becomes a page. A draft that outgrows the card opens
 * it into a full writing page; one cut back down closes it again. The two
 * thresholds are apart so a draft hovering around one of them does not make
 * the layout flip back and forth.
 *
 * | Showing | Draft lines          | Next    |
 * | ------- | -------------------- | ------- |
 * | card    | at least `openAt`    | page    |
 * | card    | fewer than `openAt`  | card    |
 * | page    | at most `closeAt`    | card    |
 * | page    | more than `closeAt`  | page    |
 */
export const pageOpenAt = 7
export const pageCloseAt = 3

/** Whether the composer should show as a page, given what it shows now and the draft's line count. */
export function nextPageMode(showingPage: boolean, lines: number): boolean {
  return showingPage ? lines > pageCloseAt : lines >= pageOpenAt
}
