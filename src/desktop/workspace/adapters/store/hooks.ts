import { useDispatch, useStore } from "react-redux"
import type { DesktopDispatch, DesktopStore } from "../../../store"

export const useWorkspaceDispatch = useDispatch.withTypes<DesktopDispatch>()
export { useWorkspaceSelector } from "./selector-paint"
/**
 * The store itself, for a handler that reads state when it runs: a view that
 * only needs a value on click should not render again every time it changes.
 */
export const useWorkspaceStore = useStore.withTypes<DesktopStore>()
