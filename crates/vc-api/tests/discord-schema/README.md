# Discord schema tests

`openapi.json` is an unmodified copy of `specs/openapi.json` from
[discord/discord-api-spec at a5df5eb81ff14b1644f7ef416d3051a90d0742d6](https://github.com/discord/discord-api-spec/tree/a5df5eb81ff14b1644f7ef416d3051a90d0742d6).
The upstream MIT license is included as `LICENSE`; `upstream-commit` records the pin.

SHA-256 of `openapi.json`:
`a6289ba51a1bdd296daa767a04d37c176b83c44577e336afb0da8d49209a2eab`

`tests/support/discord_schema.rs` selects the JSON request-body schemas for:

- Global and guild command bulk overwrite (`PUT`).
- Interaction callbacks (`POST`), also used for inline HTTP interaction responses.
- Webhook execution (`POST`), used for interaction follow-up messages.

The validator uses JSON Schema Draft 2020-12 with format validation. References
resolve against the vendored components. Network and file retrieval features of
the test-only `jsonschema` dependency are disabled.

The interaction test helper validates every successful response, including
user-facing error messages. The Discord fake validates every follow-up body.
HTTP errors such as invalid signatures are not interaction callback payloads and
are excluded. `tests/discord_schema.rs` validates the registered commands and
checks that invalid payloads are rejected.

## Upstream discrepancy

The command schema calls `default_member_permissions` an integer, but the
[Discord documentation](https://docs.discord.com/developers/interactions/application-commands#application-command-object)
defines it as a decimal string. The upstream README explicitly notes that some
documented strings are represented as integers in the spec. The helper adds a
decimal-string alternative for this property only, retaining the upstream
integer/null schema. It does not transform the payload or modify the vendored file.

These tests enforce the schema, not rules absent from it. In particular, unknown
properties remain allowed where upstream does not forbid them; interaction
timing and ACK ordering are not checked. Discord's `x-discord-union` annotation
is not a standard JSON Schema assertion, so `anyOf` retains its standard meaning.

## Updating

Choose an upstream commit, replace `openapi.json` and `LICENSE` from that commit,
and update `upstream-commit`, the link, and the checksum above. Review the schema
changes and the permissions discrepancy, then run:

```sh
nix develop --command cargo test -p vc-api --test discord_schema
nix develop --command cargo nextest run -p vc-api
```
