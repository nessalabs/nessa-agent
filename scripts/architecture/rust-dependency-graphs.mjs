/**
 * The Rust packages that must remain reusable without a desktop framework.
 *
 * Cargo package IDs, rather than dependency spellings, are followed below so a
 * renamed or transitive dependency cannot hide the package that Cargo resolves.
 */
export const PORTABLE_RUST_PACKAGES = Object.freeze([
  "nessa-agent-credentials",
  "nessa-server",
  "nessa-sdk",
  "nessa-auth",
  "nessa-local-storage",
  "nessa-images",
  "nessa-mcp",
])

function isTauriDesktopFramework(packageName) {
  return (
    packageName === "tauri" ||
    packageName.startsWith("tauri-") ||
    packageName === "tao" ||
    packageName === "wry"
  )
}

function packageLabel(pkg) {
  return `${pkg.name}@${pkg.version}`
}

function dependencyName(packageName) {
  return packageName.replaceAll("-", "_")
}

function pathLabel(packagesById, path) {
  const [root, ...steps] = path
  let label = packageLabel(packagesById.get(root.pkg))
  for (const step of steps) {
    const dependency = packagesById.get(step.pkg)
    const renamed = step.name !== dependencyName(dependency.name) ? " (renamed)" : ""
    label += ` --${step.name}${renamed}--> ${packageLabel(dependency)}`
  }
  return label
}

function pathTo(parents, packageId) {
  const reversed = []
  let current = packageId
  while (current !== undefined) {
    const parent = parents.get(current)
    reversed.push({ pkg: current, name: parent?.name })
    current = parent?.pkg
  }
  reversed.reverse()
  reversed[0] = { pkg: reversed[0].pkg }
  return reversed
}

/**
 * Return dependency paths from selected workspace packages to Tauri desktop
 * framework packages in a Cargo metadata resolve graph.
 */
export function rustDependencyGraphViolations(
  metadata,
  selectedPackageNames = PORTABLE_RUST_PACKAGES,
) {
  const packagesById = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]))
  const nodesById = new Map(metadata.resolve.nodes.map((node) => [node.id, node]))
  const workspace = new Set(metadata.workspace_members)
  const workspacePackages = metadata.packages.filter((pkg) => workspace.has(pkg.id))
  const violations = []

  for (const selectedName of selectedPackageNames) {
    const roots = workspacePackages.filter((pkg) => pkg.name === selectedName)
    if (roots.length !== 1) {
      violations.push(
        `expected exactly one workspace package named "${selectedName}", found ${roots.length}`,
      )
      continue
    }

    const root = roots[0]
    const parents = new Map([[root.id, undefined]])
    const queue = [root.id]
    for (let index = 0; index < queue.length; index += 1) {
      const packageId = queue[index]
      const node = nodesById.get(packageId)
      if (!node) {
        violations.push(`Cargo metadata has no resolve node for ${packageLabel(root)}`)
        break
      }
      for (const dependencyEdge of node.deps) {
        const dependency = packagesById.get(dependencyEdge.pkg)
        if (!dependency) {
          violations.push(
            `Cargo metadata resolve edge from ${packageLabel(packagesById.get(packageId))} points to unknown package ID ${dependencyEdge.pkg}`,
          )
          continue
        }
        if (parents.has(dependency.id)) continue
        parents.set(dependency.id, { pkg: packageId, name: dependencyEdge.name })
        if (isTauriDesktopFramework(dependency.name)) {
          violations.push(
            `"${selectedName}" reaches desktop framework package "${dependency.name}" through ${pathLabel(packagesById, pathTo(parents, dependency.id))}`,
          )
          continue
        }
        queue.push(dependency.id)
      }
    }
  }

  return violations
}
