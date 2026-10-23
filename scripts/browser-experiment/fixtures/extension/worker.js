chrome.runtime.onMessage.addListener((message, _sender, reply) => {
  if (message !== "fixture-visit") return false
  chrome.storage.local.get({ visits: 0 }).then(async ({ visits }) => {
    await chrome.storage.local.set({ visits: visits + 1 })
    reply({ visits: visits + 1 })
  })
  return true
})
