import { loadEnvironment } from "./environment"

/** Sole adapter for frontend environment configuration. VITE values are public. */
export function environmentFromVite() {
  return loadEnvironment(
    {
      VITE_NESSA_STAGE: import.meta.env.VITE_NESSA_STAGE,
      VITE_NESSA_CONVERSATION_BACKEND: import.meta.env.VITE_NESSA_CONVERSATION_BACKEND,
      VITE_NESSA_CONVERSATION_SCENARIO: import.meta.env.VITE_NESSA_CONVERSATION_SCENARIO,
    },
    import.meta.env.DEV,
  )
}
