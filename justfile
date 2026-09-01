# Nessa. The OS-shaped work lives in scripts/launch/; this file is the
# entry so we do not keep a bash script and a cmd script.
#
#   just        desktop app when a display exists; browser UI otherwise
#   just web    UI in a browser only; window controls no-op
#   just fast   testing-shaped release (macOS .app / Linux .deb / Windows nsis)

# cmd so Windows does not need Git's sh. Unix still uses sh.
set windows-shell := ["cmd.exe", "/c"]

# Desktop app when a display exists; browser UI otherwise.
default:
    node scripts/launch/cli.mjs

# UI in a browser only; window controls no-op.
web:
    node scripts/launch/cli.mjs --web

# Testing-shaped release (macOS .app / Linux .deb / Windows nsis).
fast:
    node scripts/launch/cli.mjs --fast
