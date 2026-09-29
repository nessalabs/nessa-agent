import { Fragment } from "react"
import { inlineRuns } from "../../model/transcript"

/** Prose with its `code` and **strong** runs drawn as such. */
export function RichText({ text }: { text: string }) {
  return (
    <>
      {inlineRuns(text).map((run, index) =>
        run.kind === "code" ? (
          <code key={index}>{run.text}</code>
        ) : run.kind === "strong" ? (
          <strong key={index}>{run.text}</strong>
        ) : (
          <Fragment key={index}>{run.text}</Fragment>
        ),
      )}
    </>
  )
}
