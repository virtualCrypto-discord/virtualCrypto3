#!/usr/bin/env bash
# Prepare the sqlx offline data with the compiler actually running.
#
# `cargo sqlx prepare` learns queries from the **compiler**, so a run that
# compiles nothing writes nothing — and what it writes is the whole directory, so
# "nothing" means it *empties* `.sqlx`. That is not a theoretical failure in this
# checkout:
#
#   * mbx — the shared build cache the dev shell installs in front of cargo —
#     restores artifacts by content, so an unchanged tree compiles nothing at all
#     and a bare `cargo sqlx prepare` answers "no queries found" and empties the
#     directory. Touching the sources does not help: the cache does not read
#     mtimes, so nothing is invalidated.
#   * A partial compile is worse than an empty one. Every crate that was restored
#     rather than built drops out of the data silently, and the result looks like
#     a real answer until a deployment built with `SQLX_OFFLINE` fails on a query
#     whose metadata went with it.
#
# The cure is to make the compiler run for every crate that carries a query: mbx
# is switched off for this script's own commands (`MBX_DISABLE`), and those
# crates' artifacts are removed first so nothing can be restored in their place.
# The list is discovered rather than kept by hand, so a crate that gains its first
# query needs no change here.
#
# The data is committed, so a bad write is recoverable with `git checkout --
# .sqlx` — but only by somebody who noticed. This script does not rely on that: an
# empty result puts the old data back and fails.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

# Every crate whose sources carry a query macro, by **package** name rather than
# directory name — they agree today and need not keep agreeing.
packages=()
while IFS= read -r manifest; do
  package="$(sed -n 's/^name = "\(.*\)"/\1/p' "$manifest" | head -1)"
  if [ -n "$package" ]; then
    packages+=("$package")
  fi
done < <(grep -rl --include='*.rs' 'sqlx::query' crates \
  | sed 's|/src/.*||; s|/tests/.*||' \
  | sort -u \
  | sed 's|$|/Cargo.toml|')

if [ "${#packages[@]}" -eq 0 ]; then
  echo "FAIL no crate carries a query; is this the right tree?"
  exit 1
fi

backup="$(mktemp -d)"
trap 'rm -rf "$backup"' EXIT
mkdir -p .sqlx
cp -a .sqlx/. "$backup/"

clean=()
for package in "${packages[@]}"; do
  clean+=(-p "$package")
done

echo "sqlx-prepare: compiling ${packages[*]} rather than restoring them"

# The shim, off: what restores an artifact is also what keeps the compiler from
# running, and a compiler that does not run reports no queries.
MBX_DISABLE=1 cargo clean "${clean[@]}" >/dev/null
MBX_DISABLE=1 cargo sqlx prepare --workspace -- --all-targets

prepared="$(find .sqlx -name '*.json' | wc -l | tr -d ' ')"

if [ "$prepared" -eq 0 ]; then
  echo "FAIL the compiler reported no queries; the data that was there is back"
  rm -rf .sqlx
  mkdir -p .sqlx
  cp -a "$backup/." .sqlx/
  exit 1
fi

changed="$(git status --short .sqlx | wc -l | tr -d ' ')"

if [ "$changed" -eq 0 ]; then
  echo "sqlx-prepare: $prepared queries, none changed"
else
  echo "sqlx-prepare: $prepared queries, $changed entries changed"
fi
