/** Reading the actual tool steps inside one caller-identified agent reply. */
import { css } from "./selectors.mjs"

/** Open only this message's collapsed activities; live steps need no summary. */
export async function readTranscriptSteps(message) {
  const disclosures = []
  const owners = await message.count()
  if (owners !== 1)
    return {
      labels: [],
      disclosures,
      failures: [`tool evidence has ${owners} agent messages, not one`],
    }
  for (const trigger of await message.locator(css.transcriptWorkTrigger).all()) {
    const before = await trigger.getAttribute("aria-expanded")
    if (before === "false") await trigger.click()
    disclosures.push({
      before,
      expanded: await trigger.getAttribute("aria-expanded"),
    })
  }
  return {
    labels: await message.locator(css.transcriptStepLabel).allTextContents(),
    disclosures,
    failures: [],
  }
}
