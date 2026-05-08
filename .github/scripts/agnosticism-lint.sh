#!/usr/bin/env bash
# D51 enforcement: no hardcoded profile/block/bridge name string literals
# in `specdev-analyzer/src/` (excluding `embedded/`, doc-strings, in-source
# `#[cfg(test)] mod tests` blocks, and explicitly allowlisted lines).
#
# Implements doc 21 §9 (CI lint specification).
#
# Usage:
#   .github/scripts/agnosticism-lint.sh [<plugin-tree-path>]
#
# When invoked with no arg, the script resolves the plugin tree relative
# to its own location (../../../specdev). CI workflows pass an explicit
# path after a sibling actions/checkout@v4.
#
# ============================================================================
# Allowlist mechanisms (in order of precedence):
#
# 1. PER-LINE allowlist — append ` // D51-fallback: <reason>` (or
#    ` # D51-fallback: ...` for non-Rust) to a violating source line. The
#    line is dropped from the violation set. This is the mechanism used to
#    mark intentional pre-D52 backward-compat fallbacks per doc 21 §6.5;
#    those lines are scheduled for deletion in v1.1 (step 10).
#
# 2. PER-FILE allowlist — append a file path to the FILE_ALLOWLIST array
#    below. ALL hits inside the file are silently dropped. Used for files
#    where the entire content is legitimately allowed (parser test fixtures
#    inside `#[cfg(test)] mod tests { ... }` blocks that this script
#    cannot reliably detect via line-level filtering).
#
# 3. DOC-STRING filter — lines matching `^\s*//[/!]` (Rust `///` outer or
#    `//!` inner doc comments) are dropped unconditionally. Doc-strings
#    legitimately reference profile names for narrative purposes.
#
# 4. PATH exclusion — `src/embedded/` is excluded via grep's `--exclude-dir`
#    (those YAML schemas legitimately enumerate names in `enum:` lists).
#
# Languages (`rust`, `c`, `cpp`, `python`) are NOT D51-restricted; they're
# universal infrastructure. The NAMES set excludes the `languages/` tier.
# ============================================================================

set -euo pipefail

# Per-file allowlist: every grep hit in these files is dropped. Files here
# are entirely #[cfg(test)] modules whose hits are parser test fixtures.
FILE_ALLOWLIST=(
  "src/kinds.rs"          # parser test fixtures: parse("embassy.Task") etc.
  "src/aggregator.rs"     # mod tests: profile = "firmware-layered" fixture
  "src/bootstrap.rs"      # mod tests: SpecConfig fixtures
  "src/config.rs"         # mod tests: TOML parsing fixtures
  "src/inline.rs"         # mod tests: lex_order_bridge("embassy") etc.
  "src/layout.rs"         # mod tests: SpecConfig fixtures
  "src/lineage.rs"        # mod tests: Lineage::default fixtures
  "src/plugin_root.rs"    # mod tests: SpecConfig fixtures
  "src/reductions.rs"     # mod tests: ReductionNamespace fixtures
  "src/upstream.rs"       # mod tests: UnitKind::Block fixtures
)

# ---------------------------------------------------------------------------
# Resolve plugin tree.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
default_plugin_tree="$(cd "$script_dir/../../../specdev" 2>/dev/null && pwd || true)"
plugin_tree="${1:-$default_plugin_tree}"

if [ -z "$plugin_tree" ] || [ ! -d "$plugin_tree" ]; then
  echo "agnosticism-lint: plugin tree not found at: ${1:-$default_plugin_tree}" >&2
  echo "agnosticism-lint: pass an explicit path: $0 /path/to/specdev" >&2
  exit 2
fi

# Resolve analyzer src tree. Prefer SPECDEV_ANALYZER_SRC env (used by tests
# that synthesize a temp src tree) over relative resolution.
analyzer_src="${SPECDEV_ANALYZER_SRC:-$(cd "$script_dir/../.." 2>/dev/null && pwd)/src}"
if [ ! -d "$analyzer_src" ]; then
  echo "agnosticism-lint: analyzer src not found at: $analyzer_src" >&2
  exit 2
fi

echo "agnosticism-lint: plugin tree   = $plugin_tree"
echo "agnosticism-lint: analyzer src  = $analyzer_src"
echo

# ---------------------------------------------------------------------------
# Build NAMES set from the plugin tree's HEAD.
NAMES=()
if [ -d "$plugin_tree/profiles" ]; then
  while IFS= read -r entry; do NAMES+=("$entry"); done < <(ls "$plugin_tree/profiles")
fi
if [ -d "$plugin_tree/blocks" ]; then
  while IFS= read -r entry; do NAMES+=("$entry"); done < <(ls "$plugin_tree/blocks")
fi
if [ -d "$plugin_tree/bridges" ]; then
  while IFS= read -r entry; do
    [ "$entry" = "local" ] && continue
    NAMES+=("$entry")
  done < <(ls "$plugin_tree/bridges")
fi

if [ "${#NAMES[@]}" -eq 0 ]; then
  echo "agnosticism-lint: NAMES set empty — plugin tree has no profiles/blocks/bridges?" >&2
  exit 2
fi

echo "agnosticism-lint: scanning for ${#NAMES[@]} names: ${NAMES[*]}"
echo

# ---------------------------------------------------------------------------
# Helpers.

# Build a sed-style alternation of file-allowlist relative paths.
file_allowlist_match() {
  local rel="$1"
  for allowed in "${FILE_ALLOWLIST[@]}"; do
    [ "$rel" = "$allowed" ] && return 0
  done
  return 1
}

# Drop:
#   - lines whose content (post-`file:lineno:`) starts with `//` `///` `//!`
#     i.e. Rust line / doc comments. (Block comments not handled — judged
#     not worth the complexity.)
#   - lines containing the per-line allowlist marker `D51-fallback:`.
#   - hits inside per-file-allowlisted source files.
filter_hits() {
  local hits="$1"
  local out=""
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    # Format: <abs-path>:<lineno>:<content>
    local file_path lineno content
    file_path="${line%%:*}"
    local rest="${line#*:}"
    lineno="${rest%%:*}"
    content="${rest#*:}"

    # Convert file_path to analyzer-relative for allowlist matching.
    local rel_path="${file_path#$analyzer_src/}"
    rel_path="src/$rel_path"
    if file_allowlist_match "$rel_path"; then
      continue
    fi

    # Doc-string filter (leading `//`).
    if [[ "$content" =~ ^[[:space:]]*//[/!]? ]]; then
      continue
    fi

    # Per-line allowlist marker.
    if [[ "$content" == *"D51-fallback:"* ]]; then
      continue
    fi

    out+="$line"$'\n'
  done <<< "$hits"
  printf '%s' "$out"
}

# ---------------------------------------------------------------------------
# Main scan.
violation_count=0
for name in "${NAMES[@]}"; do
  # Quoted literal grep. `--include='*.rs'` excludes non-rust files.
  raw=$(grep -rn --include='*.rs' \
    --exclude-dir=embedded \
    "\"$name\"" \
    "$analyzer_src/" 2>/dev/null || true)

  filtered=$(filter_hits "$raw")
  if [ -n "$filtered" ]; then
    echo "D51 violation: hardcoded reference to \`$name\`:"
    echo "$filtered" | sed 's/^/  /'
    echo
    violation_count=$((violation_count + 1))
  fi
done

if [ "$violation_count" -eq 0 ]; then
  echo "agnosticism-lint: clean. No D51 violations."
  exit 0
fi

echo "agnosticism-lint: $violation_count name(s) had violations." >&2
exit "$violation_count"
