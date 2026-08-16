#!/usr/bin/env bash
#
# migrate-wit-to-pact.sh — migrate a guest crate to packr-guest 0.18 (Pact naming).
#
# packr 0.18 removed the old "wit" names entirely (no wire/ABI change — pure
# source rename). Run this from a crate/repo root to do the mechanical swap:
#
#   - `wit!`                   -> `pact!`   (incl. `packr_guest::wit!`)
#   - `#[export(wit = "…")]`   -> `#[export(pact = "…")]`
#   - `#[import(… wit = "…")]` -> `#[import(… pact = "…")]`
#   - a packr `wit/` directory -> `pact/`
#   - packr `*.wit` / `*.wit+` -> `*.pact`
#
# IMPORTANT — it does NOT touch external **WASI / component-model WIT**. Those
# are real WIT (not packr's guest surface): `.wit` files with a `package ns:name`
# header, and anything under a path segment matching wasi / deprecated /
# component-model, are left alone and reported as skipped. Extend that exclude
# set with MIGRATE_EXCLUDE (an extended-regex matched against each path).
#
# It does NOT bump your Cargo.toml — set `packr-guest = "0.18"` yourself, then
# `cargo build`. Review `git diff` afterwards; `world!`/`pack_types!` are
# unchanged (they never carried the "wit" name). Idempotent; uses `git mv`.
set -euo pipefail

EXCLUDE_RE="${MIGRATE_EXCLUDE:-(^|/)(wasi[^/]*|deprecated|component-model)(/|$)}"
prune=(-not -path '*/target/*' -not -path '*/.git/*')

is_excluded() { [[ "$1" =~ $EXCLUDE_RE ]]; }

# A packr definition uses `interface`/`world`/`record`/… with NO WIT
# `package ns:name` header; standard WIT (WASI etc.) opens with a package decl.
is_packr_def() { ! grep -qE '^[[:space:]]*package[[:space:]]+[a-z0-9_-]+:' "$1"; }

mv_cmd() { git mv "$1" "$2" 2>/dev/null || mv "$1" "$2"; }

renamed=() ; skipped=()

# 1. packr `*.wit`/`*.wit+` files -> `*.pact` (skip external WIT)
while IFS= read -r -d '' f; do
  if is_excluded "$f" || ! is_packr_def "$f"; then skipped+=("$f (external WIT)"); continue; fi
  case "$f" in
    *.wit+) new="${f%.wit+}.pact" ;;
    *.wit)  new="${f%.wit}.pact" ;;
  esac
  mv_cmd "$f" "$new"; renamed+=("$f -> $new")
done < <(find . -type f \( -name '*.wit' -o -name '*.wit+' \) "${prune[@]}" -print0)

# 2. `wit/` dirs -> `pact/` (skip excluded, or dirs that still hold non-packr .wit)
while IFS= read -r -d '' d; do
  if is_excluded "$d"; then skipped+=("$d/ (external WIT)"); continue; fi
  if find "$d" -type f \( -name '*.wit' -o -name '*.wit+' \) -print -quit | grep -q .; then
    skipped+=("$d/ (still holds non-packr .wit)"); continue
  fi
  mv_cmd "$d" "$(dirname "$d")/pact"; renamed+=("$d/ -> $(dirname "$d")/pact/")
done < <(find . -type d -name wit "${prune[@]}" -print0)

# 3. Source edits — always safe: `wit!` / `wit=` are packr-specific tokens.
while IFS= read -r -d '' rs; do
  sed -i -E 's/\bwit!/pact!/g; s/([(,][[:space:]]*)wit([[:space:]]*=)/\1pact\2/g' "$rs"
done < <(find . -name '*.rs' "${prune[@]}" -print0)

echo "== wit -> pact migration =="
printf '  renamed: %s\n' "${renamed[@]:-(none)}"
printf '  skipped: %s\n' "${skipped[@]:-(none)}"
echo "Source .rs edits applied (wit! -> pact!, wit = -> pact =)."
echo 'Next: set packr-guest = "0.18" in Cargo.toml, `cargo build`, review `git diff`.'
