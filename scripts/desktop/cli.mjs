/**
 * Reading arguments, for the scripts under `scripts/desktop/`.
 *
 * Its own file because it belongs to none of them. It lived in
 * `updater-manifest.mjs`, whose stated job is "the parts of a release manifest
 * a local harness has to get exactly right" — so `release-version.mjs`, a
 * version-agreement gate with nothing to do with manifests, imported the
 * manifest module to read a flag. The import graph is how you answer "what
 * breaks if the manifest shape changes", and that made it stop answering.
 */

/** Read one `--name value` or `--name=value` argument. */
export function option(args, name, fallback) {
  const index = args.indexOf(`--${name}`)
  if (index >= 0) {
    const value = args[index + 1]
    if (!value || value.startsWith("--")) throw new Error(`--${name} requires a value`)
    return value
  }
  const equals = args.find((argument) => argument.startsWith(`--${name}=`))
  return equals ? equals.slice(name.length + 3) : fallback
}
