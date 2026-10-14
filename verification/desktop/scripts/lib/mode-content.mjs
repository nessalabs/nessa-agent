/** Compare all view content except the mode and observation metadata (#827). */
export function modeIndependentContent(view) {
  const {
    revision: _revision,
    approvalMode: _approvalMode,
    serverNowMs: _serverNowMs,
    ...content
  } = view
  return JSON.stringify(content)
}
