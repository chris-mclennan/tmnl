#!/bin/bash
# tmnl-nightly-launcher — execs the latest cargo release-build of
# tmnl from $HOME/Projects/tmnl/target/release/tmnl.
#
# Unlike mnml/mixr (which run inside tmnl), tmnl is the outer
# terminal — the launcher just execs the binary. NSApplication
# picks up our bundle's Info.plist via LaunchServices before the
# exec, so the nightly bundle identity is preserved (dock icon,
# Cmd+Tab name, etc.).

dev_bin="$HOME/Projects/tmnl/target/release/tmnl"
log_file="${TMPDIR:-/tmp}/tmnl-nightly-launcher.log"

{
  echo "----"
  echo "$(date '+%Y-%m-%d %H:%M:%S') tmnl-nightly-launcher starting"
  echo "  dev_bin=$dev_bin"
} >> "$log_file" 2>&1

if [ ! -x "$dev_bin" ]; then
    osascript <<EOF
display dialog "tmnl-nightly: no build at $dev_bin\n\nRun 'cargo build --release' in ~/Projects/tmnl first." buttons {"OK"} default button "OK" with icon caution
EOF
    exit 1
fi

exec "$dev_bin"
