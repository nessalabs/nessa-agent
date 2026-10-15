import type { ButtonHTMLAttributes, ReactNode } from "react"
import "./agent-action-card.css"

/** The quiet card an agent's next step sits in: one headline, its actions,
 * and any notes underneath. Sign-in recovery and setup (download, sign in)
 * share it so every agent prompt looks the same wherever it appears. */
export function AgentActionCard({
  label,
  title,
  actions,
  children,
  className,
}: {
  /** The card's accessible name. */
  label: string
  title: ReactNode
  actions?: ReactNode
  /** Notes and forms that follow the headline on their own lines. */
  children?: ReactNode
  className?: string
}) {
  return (
    <div
      className={className ? `agent-action-card ${className}` : "agent-action-card"}
      role="group"
      aria-label={label}
    >
      <h3>{title}</h3>
      {actions ? <div className="agent-action-card-actions">{actions}</div> : null}
      {children}
    </div>
  )
}

/** A pill action in an {@link AgentActionCard}; `quiet` for the secondary one. */
export function AgentActionButton({
  quiet = false,
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { quiet?: boolean }) {
  const classes = ["agent-action-card-button", quiet ? "is-quiet" : null, className]
  return <button type="button" className={classes.filter(Boolean).join(" ")} {...props} />
}
