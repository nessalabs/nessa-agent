/**
 * When the webview's own right-click menu opens. Anywhere the window has no
 * menu of its own, that menu — Back, Reload, Inspect Element — is a browser's,
 * not the app's, and shows nothing. Where it does belong is on text: in a
 * field it is Cut, Copy and Paste, and on selected text it is Copy and Look
 * Up. A development build keeps it one modifier away, so Inspect stays in
 * reach.
 */

/** What a right-click landed on, as far as the webview's menu is concerned. */
export interface RightClick {
  /** In a text field, a text area or editable text. */
  inEditableText: boolean
  /** On text the person has selected. */
  onSelectedText: boolean
  /** With ⌥ held. */
  withOption: boolean
}

/**
 * Whether the webview's menu opens for a right-click no menu of ours took.
 * `inspectable` is a development build, where ⌥-right-click reaches it anywhere.
 */
export function webviewMenuOpens(click: RightClick, inspectable: boolean): boolean {
  return click.inEditableText || click.onSelectedText || (inspectable && click.withOption)
}
