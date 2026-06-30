#!/usr/bin/env bash
# Two checks:
#
#   1. NAME agnosticism: no hardcoded profile/block/bridge name string
#      literals in `specdev-analyzer/src/` (excluding `embedded/`,
#      doc-strings, in-source `#[cfg(test)] mod tests` blocks, and
#      explicitly allowlisted lines).
#   2. PLANNING references: no phase nomenclature / decision-ID / doc /
#      specdev-next pointers in `src/` or `tests/` (.rs only). See the
#      "Planning-references check" section near the bottom for carve-outs.
#
# Both run every invocation; the script exits non-zero if either fails.
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
  "src/prose_lint.rs"     # mod tests: profiles/audio-plugin etc. fixture trees
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
else
  echo "agnosticism-lint: $violation_count name(s) had violations." >&2
fi
echo

# ---------------------------------------------------------------------------
# Planning-references check.
#
# The analyzer surface (its tests included) is what users and integrators
# read; internal planning nomenclature there is a dangling pointer into a
# workspace they cannot see. Ban it across `src/` and `tests/` (.rs only):
#
#   - phase nomenclature: `Phase N`, `phase_N`, `phase-N`, `phaseN`
#   - decision-ID references: `D<N>` (e.g. `D51`, `D62`)
#   - design-doc pointers: `doc N`, `specdev-next`
#
# Unlike the name check above, planning refs are banned in doc-strings too —
# that is where most of them lived. Carve-outs:
#
#   1. `// D51-fallback:` structural markers (tied to this lint; they flag
#      pre-manifest backward-compat code removed in v1.1). The `D51`
#      substring there is structural, not a citation.
#   2. A trailing `// PLANNING-OK: <reason>` per-line escape, for the rare
#      line that genuinely must carry such a token (e.g. a string fixture
#      the lint itself consumes).
#
# Languages, error-code families (`BRIDGE-LEX-001`, `TEST-KIND-001`, …), and
# dogfood finding IDs (`F-006`) are not planning pointers and do not match.
PLANNING_RE='[Pp]hase[ _-]?[0-9]|\bD[0-9]+\b|\bdoc [0-9]+|specdev-next'

analyzer_root_dir="$(dirname "$analyzer_src")"
tests_dir="$analyzer_root_dir/tests"
scan_dirs=("$analyzer_src")
[ -d "$tests_dir" ] && scan_dirs+=("$tests_dir")

planning_count=0
planning_hits=$(grep -rnE --include='*.rs' "$PLANNING_RE" "${scan_dirs[@]}" 2>/dev/null \
  | grep -vE 'D51-fallback|PLANNING-OK' || true)

if [ -n "$planning_hits" ]; then
  echo "planning-reference violation(s) — strip phase/decision/doc/specdev-next pointers"
  echo "(or escape a genuine keeper with a trailing \` // PLANNING-OK: <reason>\`):"
  echo "$planning_hits" | sed 's/^/  /'
  planning_count=$(printf '%s\n' "$planning_hits" | grep -c .)
  echo
else
  echo "agnosticism-lint: planning-references clean."
fi

# ---------------------------------------------------------------------------
# Plugin-tree planning-references check (parallel to D66 for the analyzer).
#
# The shipped plugin tree is what a user installing specdev sees; internal
# planning nomenclature there is a dangling pointer into a workspace they
# cannot read. Same ban as above, over the plugin tree's doc/config surface
# (`.md`/`.yaml`/`.yml`/`.json`). Same `PLANNING-OK` per-line escape. This is
# the guard the plugin side lacked while the analyzer side had D66 — its
# absence let ~34 refs accrete across schemas/reductions/conventions/skills
# before a manual audit caught them.
plugin_planning_count=0
plugin_planning_hits=$(grep -rnE \
  --include='*.md' --include='*.yaml' --include='*.yml' --include='*.json' \
  --exclude-dir='.git' \
  "$PLANNING_RE" "$plugin_tree" 2>/dev/null \
  | grep -vE 'PLANNING-OK' || true)

if [ -n "$plugin_planning_hits" ]; then
  echo "plugin-tree planning-reference violation(s) — the shipped plugin surface"
  echo "must carry no phase/decision/doc/specdev-next pointers (restate the rationale"
  echo "inline, or escape a genuine keeper with a trailing \` # PLANNING-OK: <reason>\`):"
  echo "$plugin_planning_hits" | sed 's/^/  /'
  plugin_planning_count=$(printf '%s\n' "$plugin_planning_hits" | grep -c .)
  echo
else
  echo "agnosticism-lint: plugin-tree planning-references clean."
fi

# ---------------------------------------------------------------------------
# Combined verdict.
total=$((violation_count + planning_count + plugin_planning_count))
if [ "$total" -eq 0 ]; then
  exit 0
fi
echo "agnosticism-lint: $violation_count name violation(s), $planning_count analyzer planning-reference violation(s), $plugin_planning_count plugin-tree planning-reference violation(s)." >&2
exit 1
