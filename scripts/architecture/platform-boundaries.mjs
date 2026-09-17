const acceptedMacosCfg = new Set([
  '#[cfg(target_os = "macos")]',
  '#[cfg(any(target_os = "macos", test))]',
])

export function hasImmediateCfg(source, declaration) {
  return acceptedMacosCfg.has(immediateCfg(source, declaration))
}

export function hasNoImmediateCfg(source, declaration) {
  return source.includes(declaration) && immediateCfg(source, declaration) === undefined
}

function immediateCfg(source, declaration) {
  const declarationAt = source.indexOf(declaration)
  if (declarationAt < 0) return undefined
  const precedingLine = source
    .slice(0, declarationAt)
    .trimEnd()
    .split("\n")
    .at(-1)
    ?.trim()
  return precedingLine?.startsWith("#[cfg(") ? precedingLine : undefined
}
