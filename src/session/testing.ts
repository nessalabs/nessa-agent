/**
 * What another context's tests may take from the session vertical: the one
 * command that puts the store's session into a given state. Tests outside this
 * folder import it from here rather than from `adapters/store/slice`. Product
 * code does not import this file.
 */
export { sessionReady } from "./adapters/store/slice"
