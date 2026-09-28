import type { ReactNode } from "react"

/**
 * The head of a side column, the same in both layouts: the titlebar row,
 * which holds only the column's own action at its far end (the window's
 * controls are the titlebar's, `WorkspaceTitlebar`), then the column's title
 * on a row of its own below — large, aligned with the column's content — and
 * whatever the column puts under it, a search field say. The title never
 * shares the titlebar row, so it can never sit under the window's controls,
 * and never has to move when a sidebar folds: the only motion is the
 * column's own slide.
 */
export function ColumnHeader({
  title,
  icon,
  subtitle,
  action,
  children,
}: {
  title?: string
  icon?: ReactNode
  subtitle?: string
  /** The column's own action, at the titlebar row's far end. */
  action?: ReactNode
  children?: ReactNode
}) {
  return (
    <>
      <div className="workspace-column-bar" data-tauri-drag-region>
        {action}
      </div>
      {title !== undefined ? (
        <div
          className="workspace-column-title"
          data-flip="slide"
          data-flip-id="column-title"
        >
          {icon}
          <h2>{title}</h2>
          {subtitle ? <p>{subtitle}</p> : null}
        </div>
      ) : null}
      {children}
    </>
  )
}
