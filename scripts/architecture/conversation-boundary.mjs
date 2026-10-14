/** The conversation's public contracts and existing composition exceptions. */
export function conversationImportAllowed(from, specifier, inComposition = false) {
  if (!/conversation/.test(specifier) || /\.css$/.test(specifier)) return true
  if (inComposition && /\/(?:adapters\/|application\/ports$)/.test(specifier)) return true
  if (
    from === "src/store.ts" &&
    /(?:^|\/)conversation\/adapters\/store\/slice$/.test(specifier)
  )
    return true
  if (from === "src/icon-preview.tsx" && /(?:^|\/)conversation\/model$/.test(specifier))
    return true
  return (
    /(?:^|\/)conversation(?:\/index)?$/.test(specifier) ||
    /(?:^|\/)conversation\/resources$/.test(specifier)
  )
}
