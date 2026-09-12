# Elixir v2 test port

Every test in `test/virtualCrypto_web/controllers/api/v2/` must have a Rust
counterpart. This table is the checklist; a row is only `ported` when all of its
cases exist and pass, and extra Rust cases are listed separately so the Elixir
coverage can still be read off at a glance.

| Elixir test file | cases | Rust test file | status |
| --- | ---: | --- | --- |
| `v2/currencies_controller_test.exs` | 15 | `tests/v2_currencies.rs` | ported (15 + 3 extra: path id, path+query, currency without assets) |
| `v2/claim/claim_controller_test.exs` | 46 | `tests/v2_claims.rs` (get by id + the per-status matrix), `tests/v2_claims_list.rs` (list), `tests/v2_claims_patch.rs` (status transitions and metadata patches), `tests/v2_claims_create.rs` (create) | ported — 4 get-by-id, a per-status × party matrix, 14 list, 19 transition/metadata and 12 create cases, plus extras for behaviour Elixir does not test |
| `v2/claim/metadata/get_test.exs` | 2 | `tests/v2_claims.rs` | ported — the metadata-scoping cases |
| `v2/claim/metadata/update_test.exs` | 15 | `tests/v2_claims_metadata.rs`, `tests/v2_claims_patch.rs`, `tests/v2_claims_create.rs` | ported — create/insert/upsert/delete/empty, updates alongside a status transition, per-user privacy, and the key, value and entry-count limits (including the trigger path) |
| `v2/user_transactions/pay/single/single_user_transaction_controller_test.exs` | 7 | `tests/v2_transactions.rs` | ported — plus a case for an unquoted idempotency key |
| `v2/user_transactions/pay/single/single_user_transaction_controller_idempotency_test.exs` | 5 | `tests/v2_transactions.rs` | ported |
| `v2/user_transactions/pay/bulk/bulk_user_transacion_controller_test.exs` | 12 | `tests/v2_transactions_bulk.rs` | ported — plus cases for entry validation and an unknown unit |
| `v2/user_transactions/pay/bulk/bulk_user_transaction_controller_idempotency_test.exs` | 1 | `tests/v2_transactions_bulk.rs` | ported |

Total: 103 Elixir cases, all ported.

The bulk path reproduces `transfer_bulk/3`'s batching rather than transferring
entry by entry: the units and the receivers are each resolved in one statement,
the sender's rows are locked once, and the receiver upsert, the sender decrement
and the history insert are one statement each. A batch therefore costs a fixed
number of round trips instead of one per entry, and the per-currency totals are
checked against the locked balances before anything is written.

## Discord interaction tests

`test/.../controllers/api/interactions/` holds 22 files and 141 cases. They are
the checklist for the interaction types that still answer 501.

| Elixir test file | cases | Rust file | status |
| --- | ---: | --- | --- |
| `common_test.exs` | 6 | `tests/interactions_common.rs` | ported (+3 extra: a forged signature, the signature covering exact bytes, a duplicated header) |
| `custom_id_test.exs` | 1 | `src/custom_id.rs` unit tests | ported (+4 extra: discriminator, unknown id, round trip) |
| `help_test.exs` | 1 | | pending |
| `invite_test.exs` | 1 | | pending |
| `pay_test.exs` | 8 | | pending |
| `bal_test.exs` | 3 | | pending |
| `info_test.exs` | 13 | | pending |
| `create_test.exs` | 9 | | pending |
| `delete_test.exs` | 3 | | pending |
| `claim/claim_make_test.exs` | 3 | | pending |
| `claim/claim_approve_test.exs` | 16 | | pending |
| `claim/claim_deny_test.exs` | 13 | | pending |
| `claim/claim_cancel_test.exs` | 13 | | pending |
| `claim/claim_show_test.exs` | 3 | | pending |
| `claim/list/claim_list_all_test.exs` | 2 | | pending |
| `claim/list/claim_list_approve_test.exs` | 16 | | pending |
| `claim/list/claim_list_deny_test.exs` | 13 | | pending |
| `claim/list/claim_list_cancel_test.exs` | 13 | | pending |
| `claim/list/claim_list_select_test.exs` | 3 | | pending |
| `claim/list/claim_list_options_test.exs` | 1 | | pending |
| `claim/list/claim_list_received_test.exs` | 0 | | empty in Elixir too |
| `claim/list/claim_list_claimed_test.exs` | 0 | | empty in Elixir too |

Total: 141 cases, 7 ported.

## Endpoints with no Elixir test

These have no test in the Elixir suite, so their contract comes from captured
responses instead (`tests/golden/`, see that directory's README):

| Endpoint | Rust test file | status |
| --- | --- | --- |
| `GET /api/v2/users/@me` | `tests/v2_users_me.rs` | ported from goldens (10 cases) |
| `GET /api/v2/users/@me/balances` | `tests/v2_balances.rs` | pending — goldens captured, endpoint not implemented |

## How to keep this honest

- The Rust test names are descriptive rather than numbered, so the mapping is
  case by case, not line by line. When a case is ported, note any deliberate
  deviation in the test itself.
- Extra Rust cases exist where a behaviour was discovered that Elixir does not
  test (for example a currency with no asset rows answering 404, or the exact
  ordering of balances). They are additive and never replace an Elixir case.
- `docs/known-gaps.md` records behaviour that is deliberately not implemented.
  A ported test is never satisfied by leaving a gap: if Elixir asserts it, the
  rewrite has to produce it.
