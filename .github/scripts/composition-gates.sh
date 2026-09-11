#!/usr/bin/env bash
# Run the per-stack composition gates.
#
# These resolve co-inlined units together and assert on the RESOLVED lockfile —
# whether two units that a project inlines side by side agree on the packages
# they share. Nothing else in the suite composes two units and resolves them;
# bridge reference crates only cover pairs a bridge happens to join, and the gap
# is the un-bridged co-inlined pair.
#
# They are `#[ignore]`d in the normal suite because they touch the network:
# crates.io, git remotes, the pub registry and the npm registry. `cargo test`
# reaches none of those; this script is the deliberate way to run the half
# that does. (`cargo test` is not fully offline — `manifest_package_json`'s
# live gates run `pnpm install` when pnpm is on PATH — but that is one
# registry, and it skips without the tool.)
#
# Requires:
#   - the plugin tree at the conventional sibling path (`../specdev`), since
#     what is under test is the units that actually ship
#   - network access to crates.io, github.com, pub.dev, and registry.npmjs.org
#   - the Flutter SDK on PATH for the dart stack (that one test skips without it)
#   - pnpm on PATH for the typescript stack (same: it skips without it)
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

# Each language stack skips when its resolver is absent, and a skip is
# invisible in the exit code — the same way an absent plugin tree would be,
# which is why that case refuses above rather than warning. Locally a warning
# is right: a contributor without the Flutter SDK should still get the four
# Rust stacks. Under CI it is not, because the job installs both tools on
# purpose, so an absent one means the job is silently gating less than its
# name claims. Warn interactively, refuse in CI.
missing=""
command -v flutter >/dev/null 2>&1 || missing="$missing flutter(dart)"
command -v pnpm    >/dev/null 2>&1 || missing="$missing pnpm(typescript)"

if [ -n "$missing" ]; then
  if [ -n "${CI:-}" ]; then
    echo "error: resolver(s) not on PATH:$missing"
    echo "       Those stacks would skip and this job would report PASS having"
    echo "       gated fewer languages than it names. The workflow installs"
    echo "       these on purpose, so an absent one is a broken job, not a"
    echo "       missing optional tool. Refusing."
    exit 2
  fi
  for tool in $missing; do
    echo "warning: $tool not on PATH — that stack will skip, not fail"
  done
fi

cargo test --test composition_stacks -- --ignored --test-threads=1
status=$?

echo "----"
if [ "$status" -eq 0 ]; then
  echo "composition gates: PASS"
else
  echo "composition gates: FAIL"
  echo "A split version means two co-inlined units disagree about a package"
  echo "they share. Fix the unit that should not be contributing it — do not"
  echo "pin around the collision in a project."
  echo "ERR_PNPM_PEER_DEP_ISSUES means a unit's package does not accept the"
  echo "version another unit contributes. Same rule: fix it in the unit."
fi
exit "$status"
