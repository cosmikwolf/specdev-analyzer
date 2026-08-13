#!/usr/bin/env bash
# Run the per-stack composition gates.
#
# These resolve co-inlined units together and assert on the RESOLVED lockfile —
# whether two units that a project inlines side by side agree on the crates they
# share. Nothing else in the suite composes two units and resolves them; bridge
# reference crates only cover pairs a bridge happens to join, and the gap is the
# un-bridged co-inlined pair.
#
# They are `#[ignore]`d in the normal suite because they touch the network:
# crates.io, git remotes, and the pub registry. `cargo test` stays hermetic;
# this script is the deliberate way to run the non-hermetic half.
#
# Requires:
#   - the plugin tree at the conventional sibling path (`../specdev`), since
#     what is under test is the units that actually ship
#   - network access to crates.io, github.com, and pub.dev
#   - the Flutter SDK on PATH for the dart stack (that one test skips without it)
#
# Run from anywhere.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"   # analyzer root (specdev-analyzer/)
cd "$ROOT" || exit 2

# Resolve the plugin tree here and hand the tests the exact path, so the script
# and the tests cannot disagree about which tree was gated. Checked in the same
# precedence order the tests use; the marker file is what makes a directory a
# plugin root.
MARKER=".claude-plugin/plugin.json"
PLUGIN=""
for candidate in "${SPECDEV_PLUGIN_ROOT:-}" "$(dirname "$ROOT")/specdev" "$ROOT/specdev"; do
  if [ -n "$candidate" ] && [ -f "$candidate/$MARKER" ]; then
    PLUGIN="$(cd "$candidate" && pwd)"
    break
  fi
done

if [ -z "$PLUGIN" ]; then
  echo "error: no plugin tree found (looked for */$MARKER in SPECDEV_PLUGIN_ROOT,"
  echo "       $(dirname "$ROOT")/specdev, and $ROOT/specdev)."
  echo "       These gates test the units that actually ship. Without the tree"
  echo "       the tests would skip and this job would go green having checked"
  echo "       nothing, so refuse instead."
  exit 2
fi
export SPECDEV_PLUGIN_ROOT="$PLUGIN"
echo "==> plugin tree: $PLUGIN"

command -v flutter >/dev/null 2>&1 \
  || echo "warning: flutter not on PATH — the dart stack will skip, not fail"

cargo test --test composition_stacks -- --ignored --test-threads=1
status=$?

echo "----"
if [ "$status" -eq 0 ]; then
  echo "composition gates: PASS"
else
  echo "composition gates: FAIL"
  echo "A split version means two co-inlined units disagree about a crate they"
  echo "share. Fix the unit that should not be contributing it — do not pin"
  echo "around the collision in a project."
fi
exit "$status"
