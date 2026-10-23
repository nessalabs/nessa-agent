const output = document.querySelector("#extension")
if (output)
  chrome.runtime.sendMessage("fixture-visit").then(({ visits }) => {
    output.textContent = `MV3 content script → service worker → chrome.storage.local: ${visits}`
    output.dataset.visits = String(visits)
  })
