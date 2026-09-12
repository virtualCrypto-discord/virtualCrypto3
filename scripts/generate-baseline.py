#!/usr/bin/env python3
"""Generate an idempotent sqlx baseline migration from `pg_dump --schema-only`.

The baseline must succeed both on a fresh empty database and on the existing
production database (where every object already exists from Ecto migrations).
We therefore rewrite each statement into an idempotent form:

  CREATE TYPE      -> wrapped in DO/EXCEPTION duplicate_object
  CREATE FUNCTION  -> CREATE OR REPLACE FUNCTION
  CREATE TABLE     -> CREATE TABLE IF NOT EXISTS
  CREATE SEQUENCE  -> CREATE SEQUENCE IF NOT EXISTS
  CREATE INDEX     -> CREATE INDEX IF NOT EXISTS
  ADD CONSTRAINT   -> wrapped in DO/EXCEPTION duplicate_object
  CREATE TRIGGER   -> preceded by DROP TRIGGER IF EXISTS

Usage: generate-baseline.py <pg_dump.sql> <out.sql>
"""

from __future__ import annotations

import re
import sys

DO_OPEN = "DO $baseline$\nBEGIN\n"
DO_CLOSE = "\nEXCEPTION WHEN duplicate_object THEN NULL;\nEND\n$baseline$;"


def preprocess(sql: str) -> str:
    """Drop psql meta-commands, comments and session settings.

    `SET search_path = ''` must not survive: it would leak into the sqlx
    session and break the unqualified `_sqlx_migrations` bookkeeping.
    """
    kept = []
    for line in sql.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("--"):
            continue
        if stripped.startswith("\\"):
            continue
        if stripped.startswith("SET "):
            continue
        if stripped.startswith("SELECT pg_catalog.set_config"):
            continue
        kept.append(line)
    return "\n".join(kept)


def split_statements(sql: str) -> list[str]:
    """Split on top-level semicolons, respecting strings and dollar quotes."""
    statements: list[str] = []
    buf: list[str] = []
    i = 0
    n = len(sql)
    while i < n:
        ch = sql[i]

        dollar = re.match(r"\$[A-Za-z_]*\$", sql[i:])
        if ch == "$" and dollar:
            tag = dollar.group(0)
            end = sql.find(tag, i + len(tag))
            if end == -1:
                buf.append(sql[i:])
                break
            buf.append(sql[i : end + len(tag)])
            i = end + len(tag)
            continue

        if ch == "'":
            j = i + 1
            while j < n:
                if sql[j] == "'":
                    if j + 1 < n and sql[j + 1] == "'":
                        j += 2
                        continue
                    break
                j += 1
            buf.append(sql[i : j + 1])
            i = j + 1
            continue

        if ch == '"':
            j = sql.find('"', i + 1)
            j = n - 1 if j == -1 else j
            buf.append(sql[i : j + 1])
            i = j + 1
            continue

        if ch == ";":
            statement = "".join(buf).strip()
            if statement:
                statements.append(statement)
            buf = []
            i += 1
            continue

        buf.append(ch)
        i += 1

    tail = "".join(buf).strip()
    if tail:
        statements.append(tail)
    return statements


def transform(statement: str) -> str:
    s = statement.strip()

    if s.startswith("CREATE TYPE "):
        return DO_OPEN + s + ";" + DO_CLOSE

    if s.startswith("CREATE FUNCTION "):
        return "CREATE OR REPLACE FUNCTION " + s[len("CREATE FUNCTION ") :]

    if s.startswith("CREATE TABLE "):
        return "CREATE TABLE IF NOT EXISTS " + s[len("CREATE TABLE ") :]

    if s.startswith("CREATE SEQUENCE "):
        return "CREATE SEQUENCE IF NOT EXISTS " + s[len("CREATE SEQUENCE ") :]

    if s.startswith("CREATE UNIQUE INDEX "):
        return "CREATE UNIQUE INDEX IF NOT EXISTS " + s[len("CREATE UNIQUE INDEX ") :]

    if s.startswith("CREATE INDEX "):
        return "CREATE INDEX IF NOT EXISTS " + s[len("CREATE INDEX ") :]

    if s.startswith("ALTER TABLE ") and " ADD CONSTRAINT " in s:
        # Guard on the catalog rather than catching an error code: re-adding a
        # primary key raises invalid_table_definition (42P16), not
        # duplicate_object (42710), so an EXCEPTION handler would miss it.
        head = re.match(r"ALTER TABLE ONLY\s+(\S+)\s+ADD CONSTRAINT\s+(\S+)", s, re.S)
        if not head:
            raise SystemExit(f"unparsable ADD CONSTRAINT: {s[:80]!r}")
        table, name = head.group(1), head.group(2)
        return (
            "DO $baseline$\nBEGIN\n"
            f"  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = '{name}'\n"
            f"      AND conrelid = '{table}'::regclass) THEN\n"
            f"    {s};\n"
            "  END IF;\n"
            "END\n$baseline$;"
        )

    if s.startswith("CREATE TRIGGER "):
        head = re.match(r"CREATE TRIGGER (\S+).*?\sON\s+(\S+)", s, re.S)
        if head:
            drop = f"DROP TRIGGER IF EXISTS {head.group(1)} ON {head.group(2)};"
            return f"{drop}\n{s}"

    return s


def main() -> int:
    source, destination = sys.argv[1], sys.argv[2]
    with open(source, encoding="utf-8") as handle:
        sql = handle.read()

    statements = [transform(s) for s in split_statements(preprocess(sql))]

    header = (
        "-- virtualCrypto baseline schema (Milestone 1).\n"
        "--\n"
        "-- Generated by scripts/generate-baseline.py from `pg_dump --schema-only`\n"
        "-- of an empty database migrated with the Ecto migrations. Every statement\n"
        "-- is idempotent so this applies cleanly both to a fresh database and to the\n"
        "-- existing production database, where the objects already exist.\n"
        "--\n"
        "-- Do not edit by hand: regenerate instead.\n\n"
    )
    with open(destination, "w", encoding="utf-8") as handle:
        handle.write(header + ";\n\n".join(statements) + ";\n")

    print(f"wrote {destination} ({len(statements)} statements)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
