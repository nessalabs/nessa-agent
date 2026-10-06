/**
 * Embeddings, overrides, isolates, and marks. Not global: a global
 * expression's `.test` keeps `lastIndex` and skips every other call.
 * `said.tsx` is what replaces them.
 */
export const bidiControls = /[\u202A-\u202E\u2066-\u2069\u200E\u200F\u061C]/
