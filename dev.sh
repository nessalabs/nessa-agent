#!/usr/bin/env bash
# Unix entry for the launch host. The path itself is scripts/launch/.
exec node "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/scripts/launch/cli.mjs" "$@"
