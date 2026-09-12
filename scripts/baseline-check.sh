#!/usr/bin/env bash
# Prove the sqlx baseline is both faithful and idempotent.
#
# Three properties are checked against a local PostgreSQL server:
#   1. applying the baseline to an EMPTY database reproduces the schema that the
#      Ecto migrations produce (scripts/schema_reference.sql);
#   2. applying the baseline a second time still succeeds (idempotent);
#   3. applying the baseline to a database that already has the Ecto schema is a
#      no-op -- this is the production cut-over case.
#
# The comparison is a pg_dump --schema-only diff, so it covers tables, columns,
# types, defaults, sequences, enums, indexes, constraints, functions and triggers.
set -euo pipefail

export PGPASSWORD="${PGPASSWORD:-postgres}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
baseline="$root/crates/vc-core/migrations/0001_baseline.sql"
reference="$here/schema_reference.sql"

psql_admin=(psql -w -X -q -v ON_ERROR_STOP=1 -h localhost -U postgres -d postgres)
psql_db() {
  local db="$1"
  shift
  psql -w -X -q -v ON_ERROR_STOP=1 -h localhost -U postgres -d "$db" "$@"
}
dump() { pg_dump --schema-only --no-owner --no-privileges -h localhost -U postgres "$1"; }
# Drop psql meta-commands (pg_dump 17 emits a random \restrict token), comments
# and session settings so only real DDL is compared.
normalize() { grep -vE '^(--|\\|SET |SELECT pg_catalog\.set_config)' || true; }

tmp="$(mktemp -d)"
cleanup() {
  "${psql_admin[@]}" -c 'DROP DATABASE IF EXISTS vc_check_fresh' >/dev/null 2>&1 || true
  "${psql_admin[@]}" -c 'DROP DATABASE IF EXISTS vc_check_existing' >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

normalize < "$reference" > "$tmp/reference.sql"
fail=0

# 1. Baseline on an empty database must reproduce the reference schema.
"${psql_admin[@]}" -c 'DROP DATABASE IF EXISTS vc_check_fresh' >/dev/null
"${psql_admin[@]}" -c 'CREATE DATABASE vc_check_fresh' >/dev/null
psql_db vc_check_fresh -f "$baseline" >/dev/null
dump vc_check_fresh | normalize > "$tmp/fresh.sql"
if diff -u "$tmp/reference.sql" "$tmp/fresh.sql" > "$tmp/fresh.diff"; then
  echo "OK   baseline on an empty database reproduces the Ecto reference schema"
else
  echo "FAIL baseline on an empty database differs from the Ecto reference schema"
  sed -n '1,100p' "$tmp/fresh.diff"
  fail=1
fi

# 2. A second run must still succeed.
if psql_db vc_check_fresh -f "$baseline" >/dev/null 2>&1; then
  echo "OK   baseline re-applies cleanly (idempotent)"
else
  echo "FAIL baseline is not idempotent"
  psql_db vc_check_fresh -f "$baseline" 2>&1 | sed -n '1,40p' || true
  fail=1
fi

# 3. Baseline on a database that already has the Ecto schema must be a no-op.
"${psql_admin[@]}" -c 'DROP DATABASE IF EXISTS vc_check_existing' >/dev/null
"${psql_admin[@]}" -c 'CREATE DATABASE vc_check_existing' >/dev/null
psql_db vc_check_existing -f "$reference" >/dev/null
if psql_db vc_check_existing -f "$baseline" >/dev/null 2>&1; then
  dump vc_check_existing | normalize > "$tmp/existing.sql"
  if diff -u "$tmp/reference.sql" "$tmp/existing.sql" > "$tmp/existing.diff"; then
    echo "OK   baseline is a no-op on an existing Ecto schema"
  else
    echo "FAIL baseline changed an existing Ecto schema"
    sed -n '1,100p' "$tmp/existing.diff"
    fail=1
  fi
else
  echo "FAIL baseline errored against an existing Ecto schema"
  psql_db vc_check_existing -f "$baseline" 2>&1 | sed -n '1,40p' || true
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "baseline-check: all checks passed"
fi
exit "$fail"
