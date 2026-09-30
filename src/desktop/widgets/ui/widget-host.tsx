import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react"
import type { WidgetRef } from "../model/widget"

interface WidgetHostValue {
  /** The widget open beside the conversation, if any. */
  readonly opened: WidgetRef | null
  /** Opens a widget beside the conversation, inside this pane. */
  open(widget: WidgetRef): void
  close(): void
  /** Opens a widget in a pane of its own, as a conversation is. */
  openPane(widget: WidgetRef): void
  /**
   * Whether a widget already has a pane of its own — a hook, so only the
   * widget that asks renders again when that changes.
   */
  useInPane(widget: WidgetRef): boolean
}

const WidgetHostContext = createContext<WidgetHostValue | null>(null)

/**
 * Where a pane keeps the widget it has open beside its conversation, and how
 * it opens one in a pane of its own (`openPane`, supplied by the workspace).
 * One per pane; a widget's inline view asks the nearest host.
 */
export function WidgetHost({
  openPane,
  useInPane,
  children,
}: {
  openPane: (widget: WidgetRef) => void
  useInPane: (widget: WidgetRef) => boolean
  children: (opened: WidgetRef | null) => ReactNode
}) {
  const [opened, setOpened] = useState<WidgetRef | null>(null)
  // Stable for the host's life, so a caller can depend on them.
  const close = useCallback(() => setOpened(null), [])
  const value = useMemo<WidgetHostValue>(
    () => ({ opened, open: setOpened, close, openPane, useInPane }),
    [opened, close, openPane, useInPane],
  )
  return (
    <WidgetHostContext.Provider value={value}>
      {children(opened)}
    </WidgetHostContext.Provider>
  )
}

/** The pane's widget host; outside a pane, a host that opens nothing. */
export function useWidgetHost(): WidgetHostValue {
  return useContext(WidgetHostContext) ?? inert
}

const inert: WidgetHostValue = {
  opened: null,
  open: () => {},
  close: () => {},
  openPane: () => {},
  useInPane: () => false,
}
