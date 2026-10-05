#!/usr/bin/env bash
# Four checks:
#
#   1. NAME agnosticism: no hardcoded profile/block/bridge name string
#      literals in `specdev-analyzer/src/` (excluding `embedded/`,
#      doc-strings, in-source `#[cfg(test)] mod tests` blocks, and
#      explicitly allowlisted lines).
#   2. PLANNING references: no phase nomenclature / decision-ID / doc /
#      planning-workspace pointers under `src/` or `tests/` — every file,
#      not just `.rs`. See the "Planning-references check" section near the
#      bottom for carve-outs.
#   3. PLUGIN-TREE planning references: the same ban over the shipped
#      plugin tree.
#   4. PLUGIN-TREE removed terms: no removed command, file or concept named
#      in the shipped plugin tree (`CHANGELOG.md` exempt).
#
# All run every invocation; the script exits non-zero if any fails.
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
# The NAME check below keeps an `--include='*.rs'` filter and that one is
# load-bearing, not an oversight: D51 bans a name baked into Rust *code*,
# while a schema `enum:` or a fixture tree is allowed to name units outright.
# The PLANNING check has no such carve-out and therefore has no filter.
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
  "src/guidance_lint.rs"  # mod tests: profiles/audio-plugin etc. fixture trees
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
# workspace they cannot see. Ban it across `src/` and `tests/`:
#
#   - phase nomenclature: `Phase N`, `phase_N`, `phase-N`, `phaseN`
#   - decision-ID references: `D<N>` (e.g. `D51`, `D62`)
#   - design-doc pointers: `doc N`
#   - planning-workspace pointers, by name and by shape (see below)
#
# #147: there was an `--include='*.rs'` filter here, and it made this check
# blind to `src/embedded/` — the eleven JSON Schemas that `include_str!`
# embeds into the shipped binary. They are as shipped as any `.rs` file and
# they carried 23 planning pointers. Proven by turning it off: a literal
# `# Phase 99 marker` appended to `src/embedded/root_kinds.schema.yaml` left
# the lint at exit 0 with 0 hits. The filter is deleted rather than extended,
# for the same reason #145 deleted the plugin half's extension list: an
# inclusion list fails silently and gets one more entry per file type. The
# `tests/` fixture trees come into scope with it — that is intended, and it
# cost 32 further edits the original report had not measured.
#
# `-I` skips binary files without naming them. Measured: no file under
# `src/` or `tests/` is binary today, so it is a safety net rather than a
# live need — it is here so a fixture that gains one cannot turn this gate
# red on bytes nobody wrote. The walk needs no git: the analyzer's
# `.gitignore` covers only `/target`, which is outside both scan roots, so
# a plain walk and `git ls-files` see the same set here. That is NOT true
# on the plugin side, where an installed `node_modules/` is ignored and
# must drop out — hence the two halves scan differently.
#
# The workspace-name term is TWO terms, deliberately:
#
#   - `specdev-(next|planning|lab)` — the workspace has had three names, and
#     the pattern previously banned only the first. A rename silently
#     disarmed it for two renames running.
#   - `[0-9]{2}-<name>.md` — the rename-proof half, and the one that matters.
#     Every planning doc is a numbered markdown file (`10-metaschema.md`,
#     `11-issues.md`), so this catches the pointer whatever the directory is
#     called. Measured: 0 false positives across the analyzer and the plugin
#     tree. A bare `specdev-*` pattern is NOT usable — `specdev-analyzer` is
#     a legitimate name with 115 occurrences, and the test temp-dir prefixes
#     (`specdev-merge-`, `specdev-hello-rtic-`, …) add ~90 more.
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
#      CAUTION, now that YAML is in scope: inside a block scalar
#      (`description: >` or `description: |`) a trailing `# PLANNING-OK:`
#      is NOT a comment — YAML folds it into the string value. Verified
#      with a parser: the description then ends `... # PLANNING-OK: ...`.
#      So in a block scalar, STRIP the pointer; do not escape it. The
#      escape is safe on a `#`-comment line, a TOML line, and Rust code.
#
# Languages, error-code families (`BRIDGE-LEX-001`, `TEST-KIND-001`, …), and
# dogfood finding IDs (`F-006`) are not planning pointers and do not match.
PLANNING_RE='[Pp]hase[ _-]?[0-9]|\bD[0-9]+\b|\bdoc [0-9]+|specdev-(next|planning|lab)\b|\b[0-9]{2}-[a-z][a-z-]*\.md'

analyzer_root_dir="$(dirname "$analyzer_src")"
tests_dir="$analyzer_root_dir/tests"
scan_dirs=("$analyzer_src")
[ -d "$tests_dir" ] && scan_dirs+=("$tests_dir")

planning_count=0
planning_hits=$(grep -rnEI "$PLANNING_RE" "${scan_dirs[@]}" 2>/dev/null \
  | grep -vE 'D51-fallback|PLANNING-OK' || true)

if [ -n "$planning_hits" ]; then
  echo "planning-reference violation(s) — strip phase/decision/doc/workspace pointers"
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
#
# The file list comes from git, not from a directory walk, and that is
# load-bearing. A `reference/typescript/` package installs its
# dependencies into `node_modules/`, and those carry thousands of
# `.md`/`.json` files written by other people — one `D3` in a d3-geo
# README reads as a decision pointer and turns this gate red on content
# the plugin does not ship. CI installs them, so the walk breaks there,
# not only on a developer's machine.
#
# `--cached --others --exclude-standard` is tracked files PLUS untracked
# files that `.gitignore` does not cover. That is exactly "the shipped
# surface": a build artefact or an installed dependency is ignored and
# drops out by construction, while a file authored two minutes ago and
# not yet `git add`ed is still scanned — so the fix does not buy silence
# on new work, which would be the false green this gate exists to stop.
# An enumerated `--exclude-dir` list was the alternative and was
# rejected: the next ecosystem brings the next directory name, and a
# missing entry in an exclusion list fails silently.
#
# The walk is kept as a fallback for a plugin tree that is not a git
# checkout (a packaged install), where nothing untracked exists anyway.
#
# #145: there was an extension filter here — `.md|.yaml|.yml|.json` — and it
# covered 279 of 522 tracked files while reporting clean over **17** real
# violations. The worst were the profile `*.toml.template` files, which
# `bootstrap` copies verbatim into every new project, so a co-worker's own
# `specs/architecture.toml` opened with `# Phase 6.7+:`. Note that extending
# the list would not have fixed it either: those files end in `.template`,
# not `.toml`. An inclusion list fails silently and gets one more entry per
# ecosystem, so the list is deleted rather than extended (#122's lesson, and
# the same shape as the `--exclude-dir` argument above). Everything git
# tracks is scanned; `grep -I` skips the one binary file
# (`profiles/nextjs-app/templates/gitkeep.template`) without naming it. The
# per-line `PLANNING-OK` escape is the exception list, and it fails loudly
# because each exception sits at its own site.
plugin_planning_count=0
if git -C "$plugin_tree" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  plugin_planning_hits=$(git -C "$plugin_tree" ls-files --cached --others --exclude-standard \
    | sed "s|^|$plugin_tree/|" \
    | tr '\n' '\0' \
    | xargs -0 -r grep -HnEI "$PLANNING_RE" 2>/dev/null \
    | grep -vE 'PLANNING-OK' || true)
else
  plugin_planning_hits=$(grep -rnEI \
    --exclude-dir='.git' --exclude-dir='node_modules' --exclude-dir='target' \
    "$PLANNING_RE" "$plugin_tree" 2>/dev/null \
    | grep -vE 'PLANNING-OK' || true)
fi

if [ -n "$plugin_planning_hits" ]; then
  echo "plugin-tree planning-reference violation(s) — the shipped plugin surface"
  echo "must carry no phase/decision/doc/workspace pointers (restate the rationale"
  echo "inline, or escape a genuine keeper with a trailing \` # PLANNING-OK: <reason>\`):"
  echo "$plugin_planning_hits" | sed 's/^/  /'
  plugin_planning_count=$(printf '%s\n' "$plugin_planning_hits" | grep -c .)
  echo
else
  echo "agnosticism-lint: plugin-tree planning-references clean."
fi

# ---------------------------------------------------------------------------
# Removed-terms check over the plugin tree.
#
# The analyzer once generated code: `/spec-gen`, the `gen-*` ledgers, the
# `--skeleton` handoff, codegen prose, reference crates and their checks,
# per-block `deps.yaml`, and the subcommands that served them. All of it is
# gone. A skill or a guidance file that still names one sends the agent
# after a command or a file that does not exist, and no other gate sees it.
#
# The list lives here and not in `plugin-lint`: it is this project's
# history, not a rule for third-party plugin authors. `tests/cli_help_text.rs`
# reads this line and runs the same list over every subcommand's long help,
# so keep the pattern on one line as `REMOVED_TERMS_RE='…'`.
#
# Measured legitimate and deliberately not matched: the kept decision field
# `codegen_include`, flutter_rust_bridge's own `flutter_rust_bridge_codegen`,
# and plain "hand-edit" wording about other tools' generated files.
#
# `CHANGELOG.md` is exempt: it is where a removal is recorded. A line that
# must name a removed term (a migration note for old trees) takes a trailing
# `REMOVED-TERM-OK: <reason>` escape.
REMOVED_TERMS_RE='/spec-gen|spec-gen\b|gen-lineage|gen-drift|gen-plan|gen-record|--skeleton|HANDOFF\.md|codegen prose|codegen/|module\.conventions|codegen_unblocked|[Rr]eference[- ]crate|manifest --write|reference_cases|REF-(COVERAGE|ARITY|PIN)|CONTEXT-100|deps\.yaml|specdev-analyzer (gen|prompt|derive|manifest|cargo-toml)\b'

if git -C "$plugin_tree" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  removed_hits=$(git -C "$plugin_tree" ls-files --cached --others --exclude-standard \
    | grep -v '^CHANGELOG\.md$' \
    | sed "s|^|$plugin_tree/|" \
    | tr '\n' '\0' \
    | xargs -0 -r grep -HnEI "$REMOVED_TERMS_RE" 2>/dev/null \
    | grep -v 'REMOVED-TERM-OK' || true)
else
  removed_hits=$(grep -rnEI \
    --exclude-dir='.git' --exclude-dir='node_modules' --exclude-dir='target' \
    "$REMOVED_TERMS_RE" "$plugin_tree" 2>/dev/null \
    | grep -v "^$plugin_tree/CHANGELOG\.md:" \
    | grep -v 'REMOVED-TERM-OK' || true)
fi

removed_count=0
if [ -n "$removed_hits" ]; then
  echo "plugin-tree removed-term violation(s) — the text names a removed command,"
  echo "file or concept (rewrite it to what ships now, or escape a genuine keeper"
  echo "with a trailing \`REMOVED-TERM-OK: <reason>\`):"
  echo "$removed_hits" | sed 's/^/  /'
  removed_count=$(printf '%s\n' "$removed_hits" | grep -c .)
  echo
else
  echo "agnosticism-lint: plugin-tree removed-terms clean."
fi

# ---------------------------------------------------------------------------
# Combined verdict.
total=$((violation_count + planning_count + plugin_planning_count + removed_count))
if [ "$total" -eq 0 ]; then
  exit 0
fi
echo "agnosticism-lint: $violation_count name violation(s), $planning_count analyzer planning-reference violation(s), $plugin_planning_count plugin-tree planning-reference violation(s), $removed_count plugin-tree removed-term violation(s)." >&2
exit 1
