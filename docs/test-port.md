# Elixir test port

`virtualCrypto2` has **36 test files and 332 cases**. This is the ledger for all
of them: every case either has a Rust counterpart or is recorded here as dropped,
with the reason.

| Area | cases | ported | status |
| --- | ---: | ---: | --- |
| v2 REST API | 103 | 103 | complete |
| Discord interactions | 141 | 141 | complete |
| Notifications | 11 | 11 | complete |
| v1 REST API | 77 | 0 | dropped by decision |
| **total** | **332** | **255** | |

A row is only `ported` when all of its cases exist and pass, and extra Rust cases
are listed separately so the Elixir coverage can still be read off at a glance.

## v2 REST API

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

The bulk path reproduces `transfer_bulk/3`'s batching rather than transferring
entry by entry: the units and the receivers are each resolved in one statement,
the sender's rows are locked once, and the receiver upsert, the sender decrement
and the history insert are one statement each. A batch therefore costs a fixed
number of round trips instead of one per entry, and the per-currency totals are
checked against the locked balances before anything is written.

## Discord interactions

`test/.../controllers/api/interactions/` holds 22 files and 141 cases.

| Elixir test file | cases | Rust file | status |
| --- | ---: | --- | --- |
| `common_test.exs` | 6 | `tests/interactions_common.rs` | ported (+3 extra: a forged signature, the signature covering exact bytes, a duplicated header) |
| `custom_id_test.exs` | 1 | `src/custom_id.rs` unit tests | ported (+4 extra: discriminator, unknown id, round trip) |
| `help_test.exs` | 1 | `tests/interactions_commands.rs` | ported |
| `invite_test.exs` | 1 | `tests/interactions_commands.rs` | ported |
| `pay_test.exs` | 8 | `tests/interactions_pay.rs` | ported (+1 extra: paying a receiver with no account) |
| `bal_test.exs` | 3 | `tests/interactions_bal.rs` | ported |
| `info_test.exs` | 13 | `tests/interactions_info.rs` | ported |
| `create_test.exs` | 9 | `tests/interactions_create.rs` | ported |
| `delete_test.exs` | 3 | `tests/interactions_delete.rs` | ported |
| `claim/claim_make_test.exs` | 3 | `tests/interactions_claim.rs` | ported |
| `claim/claim_approve_test.exs` | 16 | `tests/interactions_claim.rs` | ported |
| `claim/claim_deny_test.exs` | 13 | `tests/interactions_claim.rs` | ported |
| `claim/claim_cancel_test.exs` | 13 | `tests/interactions_claim.rs` | ported |
| `claim/claim_show_test.exs` | 3 | `tests/interactions_claim.rs` | ported |
| `claim/list/claim_list_all_test.exs` | 2 | `tests/interactions_claim.rs` | ported |
| `claim/list/claim_list_approve_test.exs` | 16 | `tests/interactions_claim.rs` | ported |
| `claim/list/claim_list_deny_test.exs` | 13 | `tests/interactions_claim.rs` | ported |
| `claim/list/claim_list_cancel_test.exs` | 13 | `tests/interactions_claim.rs` | ported |
| `claim/list/claim_list_select_test.exs` | 3 | `tests/interactions_claim.rs` | ported |
| `claim/list/claim_list_options_test.exs` | 1 | `src/claim_list.rs` unit tests | ported |
| `claim/list/claim_list_received_test.exs` | 0 | | empty in Elixir too |
| `claim/list/claim_list_claimed_test.exs` | 0 | | empty in Elixir too |

The builders the interaction tests share — `execute_from_guild/2` and its
component, select, button and modal siblings, plus `setup_money/1` and
`get_amount/2` — live in `tests/support`, so a test reads like its Elixir
counterpart. `setup_money/1` inserts the rows its Elixir version ends up with
rather than calling the domain functions, which keeps each test's data explicit
and its failures readable.

The `give` command has **no Elixir test at all**; it is exercised only through
`setup_money/1` calling `Money.give/1`. `tests/interactions_give.rs` therefore
holds additions rather than ports: they follow `Command.handle/4` and
`Query.Issue.issue/3` directly.

## Notifications

`test/virtualCrypto/notification/` covers what `Notification.Dispatcher` sends
to a claimant's application when a claim is approved or denied, through a
`NotificationSink` test double. `vc_core::notification::Notifier` is that seam,
and the payloads are pinned in `tests/notification.rs`. Delivering them needs the
application side of the domain, which is not built yet — see
`docs/known-gaps.md`.

| Elixir test file | cases | Rust file | status |
| --- | ---: | --- | --- |
| `notification/single_test.exs` | 5 | `tests/notification.rs` | ported |
| `notification/bulk_test.exs` | 6 | `tests/notification.rs` | ported — with `vc_core::claim::update_claims/2` |

The bulk cases needed `update_claims/2`, which groups the requests by the status
they move to, checks the operator against every claim, moves an approval batch in
one transfer, and dispatches one notification per claimant. That is the same call
the claim-list buttons make, so it is not work spent only on these tests.

## v1 REST API: dropped

`test/.../controllers/api/v1/` holds four files and 77 cases. The v1 API is not
being carried over — the decision was that v1 may be abolished while v2 may not
be broken — so the routes are absent from the router and nothing here is ported.

| Elixir test file | cases |
| --- | ---: |
| `v1/info_controller_test.exs` | 15 |
| `v1/claim_controller_test.exs` | 43 |
| `v1/single_user_transaction_controller_test.exs` | 7 |
| `v1/bulk_user_transacion_controller_test.exs` | 12 |

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
