# QA: what a machine can check, and what a person has to

This is the verification plan for the whole service, split by **who can settle a
question**:

- **Static** (§1) — a reader, human or model, comparing one artefact against
  another. No execution, no environment. This is the section a reviewing model can
  do on its own.
- **Dynamic** (§2) — a shell and a database: the gates, the focused suites, and the
  experiments that need rows and concurrent connections.
- **Human** (§3) — Discord, a real webhook, a deployment, and judgement.

§4 lists what this document deliberately does not settle; the appendices map the
recent changes and the project's other notes.

**The one rule for every item**: it names a file, a test or a command, and a reader
can falsify it. An item that only says "check that X is right" is a defect in this
document — report it as such.

**How to report what you find**:

- A **defect**: the item, the file and line that contradict it, and the smallest
  command or read that shows it. Prefer a failing assertion over a paragraph.
- An **accepted difference** from the Elixir service this is a rewrite of: it
  belongs in `docs/known-gaps.md`.
- A **judgement call** (a number, a wording, a UX choice): §4, with a
  recommendation and what it costs.
- A **stale claim in this document**: quote it and say what the tree says instead.

**If you have one pass and not a day**, in this order:

1. §1.2 against `routes/mod.rs`, `routes/v2/mod.rs`, and the test files: the route
   table and the suite column — it is the fastest way to see a surface nothing
   holds.
2. §1.5: re-derive the reachability table from `routes/idempotency.rs` and
   `vc_core/idempotency.rs`. The interesting claim is the *unreachable* one.
3. §1.4: the two pagination mechanisms, in the code and in the docs, and the doc ↔
   code rows of that table.
4. §2.2: pick two mutations and run them. A test that cannot fail is worth less
   than no test, and this is the only cheap way to know.
5. §4: nothing there is a defect, but a recommendation with a cost is the most
   useful thing a reviewer can leave.

## 0. Orientation, for a reader who was not in the room

virtualCrypto is a Discord application and a REST API for virtual currency: guilds
issue currency from a pool, users pay each other, claims move money when two people
agree, and **contracts** let an application operate a user's currency with that
user's approval. It is a Rust rewrite of an Elixir service and **shares its
PostgreSQL schema**, which is why migrations are additive and why the Elixir's
behaviour is a reference point throughout.

| Crate | What it is |
| --- | --- |
| `vc-core` | the domain and the SQL: users, currencies, assets, claims, payments, contracts, grants, applications, idempotency, purge |
| `vc-auth` | tokens: user and application JWTs, scopes, the Discord session, the extractors |
| `vc-api` | HTTP routes, the Discord interaction surface, the docs/help join, the scheduler, notifications |
| `vc-server` | the process: configuration, the pool, the router, the clock |
| `vc-demo-app` | two sample applications that talk to the API over HTTP |

Running anything: the dev shell is Nix (`nix develop`, or direnv), the database is
PostgreSQL 17 (`just db-create && just migrate`), and the gates are `just check`
(formatting, clippy with warnings denied, the suite, the sqlx data, the baseline).
`DATABASE_URL` is exported by the justfile.

The invariants that matter most, and that most items below are about:

1. **Money is conserved.** A lock is a transfer into the contract's own account,
   never a deletion; a refusal, a withdrawal and an expiry return every remainder;
   `SUM(assets.amount)` does not move except by issuing.
2. **One key, one write.** An `Idempotency-Key` is spent by a write and only by a
   write, and a claimed key always ends with an answer.
3. **A page is bounded**, whatever the caller asks.
4. **A caller who may not see something is answered 404, not 403**, so that ids
   cannot be probed.

## 1. Static

### 1.1 The method

Three questions per artefact, in this order:

1. **Is it there?** (a route is registered, a command is dispatched, a doc section
   exists, a test names the thing.)
2. **Does it say what the code does?** (prose against behaviour, not prose against
   intent.)
3. **Would anything notice?** (a test, a golden, a refusal — the tables below name
   what.)

Then falsify: for each claim, name the read or command that would refute it. Claims
here are cheap to check with `grep` because the code says what it means — a claim
that needs a paragraph of interpretation is usually a finding.

### 1.2 The HTTP surface

Every route, who may call it, and the suite that would notice it breaking. **A
suite of "—" is either covered indirectly or a finding**; the two marked below are
findings.

| Endpoint | Caller | Suite |
| --- | --- | --- |
| `GET /health` | — | `web` |
| `GET /login`, `GET /logout`, `GET /callback/discord`, `GET /invite`, `GET /support`, `POST /token` | session | `login`, `oauth2_token_endpoint` |
| `GET /assets` | — | `web` |
| `POST /api/integrations/discord/interactions` | Ed25519 signature | `interactions_*` (eleven suites, plus a shared helper), `discord_schema` |
| `GET`/`POST /oauth2/authorize` | session | `oauth2_authorize`, `oauth2_preauthorize` |
| `POST /oauth2/token` (four grants) | client credentials | `oauth2_token`, `oauth2_token_endpoint`, `grant_requests` |
| `POST /oauth2/token/revoke` | — | **no test (finding)** |
| `GET /oauth2/clients` | app token | `oauth2_clients_mine` |
| `POST /oauth2/clients` | user token | `oauth2_clients` |
| `GET /oauth2/clients/@me` | app token | `oauth2_clients_me`, `webhook_reverify` |
| `PATCH /oauth2/clients/@me` | app token | **no test through HTTP (finding)** — only `vc_core::application::patch` is tested |
| `GET`/`POST /oauth2/clients/@me/grant-requests` | app token | `grant_requests` |
| `POST /applications/{id}/connect` | session | `connect` |
| `GET /applications/{id}/grants`, `DELETE /applications/{id}/grants/{guild_id}` | session | `guild_grants` |
| `GET /api/v2/users/@me` | user token | `v2_users_me` (+ goldens) |
| `GET /api/v2/users/@me/balances` | user token | `v2_users_me_balances` (+ goldens) |
| `GET`/`POST /api/v2/users/@me/claims` | user token + `vc.claim` | `v2_claims_list`, `v2_claims_create` |
| `GET`/`PATCH /api/v2/users/@me/claims/{id}` | `vc.claim` | `v2_claims`, `v2_claims_patch`, `v2_claims_metadata` |
| `POST /api/v2/users/@me/transactions` | `vc.pay` | `v2_transactions`, `v2_transactions_bulk` |
| `GET /api/v2/users/@me/contracts` | user token | `contracts`, `contract_pagination` |
| `GET`/`POST /api/v2/contracts` | app token + `vc.contract` | `contracts`, `contract_pagination` |
| `GET /api/v2/contracts/{id}` | app or party | `contracts` |
| `GET /api/v2/contracts/{id}/balances` | app token | `contracts` |
| `POST`/`DELETE /api/v2/contracts/{id}/approval`, `POST …/refusal` | party | `contracts`, `contract_notification` |
| `GET`/`POST /api/v2/contracts/{id}/payments` | app token (GET also party) | `contract_payments`, `contract_idempotency`, `contract_metered`, `contract_party`, `contract_escrow`, `contract_expiry` |
| `GET /api/v2/currencies`, `GET /api/v2/currencies/{id}` | — | `v2_currencies` |
| `POST /api/v2/currencies/issue` | guild token + `vc.issue` | `v2_issue` |
| `GET /api/docs` | — | `documentation` |
| `GET /` and the client routes | — | `web` |

- [ ] Every path in `crates/vc-api/src/routes/mod.rs` and `routes/v2/mod.rs` is a
      row here:
      `grep -hoE '"/[a-zA-Z0-9/:{}@_.-]*"' crates/vc-api/src/routes/{mod.rs,v2/mod.rs} | sort -u`
      is the list to compare against.
- [ ] For each endpoint, the auth column against its extractor: `AuthUser` (auth
      only), `Limited` (auth plus the per-account limit), `GuildToken` (a grant).
      `routes/limited.rs` says what is limited; a v2 handler taking `AuthUser`
      instead of `Limited` is an unlimited endpoint and a finding.
- [ ] The two findings above: `POST /oauth2/token/revoke` is what a client calls
      when a token leaked, and `PATCH /oauth2/clients/@me` is the only way an
      application changes its own webhook. Either add the test, or say in §4 why
      not.
- [ ] `Accept` under `/api` (`require_json_accept` in `routes/mod.rs`): a header
      that cannot be satisfied with JSON is a 406 before any handler runs, a
      **missing** header is treated as `*/*` and allowed, and media type parameters
      are ignored (so `q=0` excludes nothing). The four cases are goldens:
      `v2_users_me_accept_absent`, `…_accept_any`, `…_accept_html`,
      `…_accept_json_and_html`.
- [ ] Unknown fields are ignored and added response fields are not breaking
      (`docs/known-gaps.md` quotes the specification this was written against). No
      handler refuses a body for carrying a field it does not know.

### 1.3 The Discord surface

Twelve commands — `help`, `invite`, `application`, `issue`, `grant`, `contract`,
`pay`, `info`, `create`, `delete`, `bal`, `claim` — and each is four artefacts that
have to agree:

| Artefact | Where | What to check |
| --- | --- | --- |
| the registration | `discord_commands.rs` | names, descriptions, options, `contexts`, `integration_types`, the admin bit |
| the dispatch | `command/mod.rs`'s `handle` | one arm per command; an unknown name is refused |
| the prose | `docs/commands.rs` (shown by `/help` and the site) | describes what the handler does, not what it was meant to |
| the suite | `tests/interactions_*.rs` | nine commands have their own file; `help`, `invite` and `application` live in `interactions_commands.rs`, with `interactions_autocomplete.rs` and the shared `interactions_common.rs` beside them |

- [ ] `tests/documentation.rs` asserts registration ↔ prose ↔ order; a reader checks
      the *content* of the prose against the handler.
- [ ] Every component's state travels in its `custom_id`
      (`crates/vc-api/src/custom_id.rs`): a head byte per space (`0xF0` claim list,
      `0xF2` contract, others beside them), an action byte, and a decimal payload. A
      forged or stale id is refused rather than trusted.
- [ ] The contract space's ids are `Pressed::Decided(Action, id)` and
      `Pressed::Paged(Page, number)`: the same encoding for a contract id and a page
      number, told apart by the action byte. Both round-trip in `custom_id.rs`'s
      unit tests.
- [ ] The screens nobody clicks in a test: the empty list, "this page is empty", the
      error screens (`CommandError`), the ephemeral flags, the autocomplete lists
      (`command/autocomplete.rs`).
- [ ] `contexts` in the registration: `[0, 1]` everywhere except `issue`, `grant`,
      `create` and `delete`, which are `[0]` — guild only, because they act on a
      guild's currency. `discord_commands.rs`'s header says that is the whole of
      what the deprecated `dm_permission` said, and its unit test asserts the four.

### 1.4 The joins between artefacts

| This | must agree with | How it is held |
| --- | --- | --- |
| `docs/api.rs` | the routes | `tests/documentation.rs` both ways; a reader checks the content |
| `docs/contracts.md` | the contract handlers | prose only — the three rules under "Retrying a charge" are `routes/idempotency.rs`'s `guard`; the return rule is `contract::pay_in`; the sizes are `PER_PAGE`, `MAX_LIMIT`, `MAX_CONTRACTS` |
| `docs/known-gaps.md` | the tree | prose only; every claim is a file or a test |
| `docs/issue.md`, `docs/oauth2.md` | the issue endpoint, the OAuth2 surface | prose only |
| `docs/test-port.md` | the ported suites | a ledger: every Elixir case is a file here or recorded as dropped |
| `docs/web-ui.md` | `web/` | every page it fetches is a route |
| `README.md` | `scripts/sqlx-prepare.sh`, `flake.nix` | the mbx note is the script's own header |
| `.github/workflows/ci.yml` | `justfile`'s `check` | the same gates, in the same order |
| commit messages | their diffs | newest first; each message makes specific claims |

- [ ] Walk each row. The machine-checked ones fail loudly; the prose ones are what a
      reader is for.
- [ ] **Two pagination mechanisms, both kept**: the API pages with a cursor
      (`next`/`on_next`, `vc_core::page::Cursor`, `of_party`/`of_application`), the
      Discord screens page with numbers (`claim::list_page`,
      `contract::open_of_party`). Confirm neither has replaced the other, and that
      each is used where it is right: a cursor is stable under inserts and cannot
      say "back"; a page number can say first/last and shifts when rows are
      inserted.

### 1.5 Branches, and what makes each reachable

An unreachable branch that nothing names is a lie about the code; a reachable one
that nothing tests is a hole. The idempotency layer, the most recent and the most
intricate:

| Branch | Reachable | What makes it so | Pinned by |
| --- | --- | --- | --- |
| the key is claimed | yes | an ordinary first request | `a_repeated_key_charges_once`, `concurrent_charges_with_one_key_write_once` |
| the key has an answer | yes | a retry | the same two (`Duplicate`, same body) |
| the write failed | yes | a database failure (the table is dropped in the test) | `a_failure_gives_the_key_back` |
| the wait ran out | yes | another transaction holds the row for `CLAIM_WAIT` | `a_key_another_request_is_holding_answers_come_back` |
| a row that answers nothing | yes, *as data* | rows claimed by the version before `4435878` | `a_row_that_answers_nothing_asks_the_caller_to_come_back` |
| refused by the isolation level | **no** | the transaction names `READ COMMITTED` | `a_stricter_session_default_does_not_change_the_answer` |
| a key that is unquoted, too long, or several | yes | a client's mistake | `an_unquoted_key_is_rejected`, `an_over_long_key_is_refused`, `two_keys_at_once_are_refused` |

- [ ] Re-derive the table from `routes/idempotency.rs` and
      `vc_core/idempotency.rs`: every `match` arm appears, and the one marked
      unreachable has a reason a reader can check rather than "should not happen".
- [ ] The same pass over the rest: `contract::pay_in`'s error arms,
      `Page::asked`'s bounds, `scheduler::reverify_webhooks`'s handshake outcomes,
      `notification::check_webhook`'s transport choice, `interactions.rs`'s type
      dispatch, `oauth2_token`'s four grants.
- [ ] Nothing shaped like a defensive branch without a name (`unwrap_or_default`, a
      catch-all `_` arm, a "just in case" check) fails this question: **what makes
      it reachable?**

### 1.6 Invariants

- [ ] **Money is conserved.** Read `contract::approve`, `pay_in` and `end`: no path
      writes `remaining` without moving `assets` in the same transaction.
      `tests/contract_escrow.rs`'s `supply()` and `escrow()` state it.
- [ ] **A lock is a transfer.** `contract::approve` credits the contract's own
      account (`users.contract_id`); the `assets` trigger deletes a balance that
      reaches zero.
- [ ] **A claim's money**: refusal, withdrawal and expiry each return every
      remainder; a spend after the deadline is refused whether or not the clock has
      run.
- [ ] **One key, one write**: `docs/contracts.md`'s rules are the statement; the
      code is `guard` (claim, write and answer in one transaction).
- [ ] **A page is bounded**: `PER_PAGE` when the caller says nothing, `MAX_LIMIT`
      when they say too much, `MAX_CONTRACTS` on the contract screen,
      `MAX_COLUMN_COUNT` on the claim screen. `Page::next_cursor` is the only place
      that decides "there is more".
- [ ] **Error bodies have one spelling**: `ApiError::parts()` against the
      hand-written bodies in `payment_error`, `issue_error`, `contract_error`.
- [ ] **Every `_in` write runs in a transaction the layer owns**:
      `grep -n "begin()" crates/vc-core/src/{contract,payment,issue}.rs` — the
      wrappers open one, the `_in` functions never do.
- [ ] **Invisibility is 404**: a caller who is neither the application nor a party
      is answered as a contract that is not there, for reads and for writes.

### 1.7 Ported fidelity, and where it was dropped on purpose

- [ ] The goldens (`crates/vc-api/tests/golden/`: thirteen `v2_users_*.json` files
      captured from the Elixir, with `fixture.json` and a `README.md` beside them);
      `assert_matches_golden` compares status, parsed body and content type exactly.
      No fixture has been edited to fit the code.
- [ ] `docs/test-port.md`: every Elixir case is a file here or recorded as dropped,
      with a reason.
- [ ] `docs/known-gaps.md`'s differences from the Elixir, one by one — every `###`
      heading in that section, against the code: a client's typo is a 400 where the
      Elixir answered 500 (the negative limit, the ceiling on `limit`, the cursor,
      the order), the claim is in the write's transaction, `/issue` rather than
      `/give`, 303 rather than 302, `POST /token` without a session is 401, a
      member's unknown role id is skipped, the consent screen's two repairs,
      `POST /oauth2/token`'s 400, and what a delivery and a handshake check are.
- [ ] **Vacuous assertions**: a test that would pass if its subject were broken. The
      shape to look for is an id compared against the wrong account — one existed
      here (a key counted for an application's row id instead of its account id, so
      the count was always zero) — or a loop that never runs.

### 1.8 Coverage: known holes, and how to find more

- [ ] The two findings in §1.2.
- [ ] `PATCH` and `DELETE` through HTTP:
      `grep -rn '"PATCH"\|"DELETE"' crates/vc-api/tests` — comparing that list
      against §1.2 is how the `PATCH /oauth2/clients/@me` gap was found.
- [ ] Public surface with no caller: a `pub fn` in `vc-core` used once, by the thing
      written to use it, is a shape that has already been reshaped in this tree
      (`open_of_party`). Prefer the read the caller actually needs.
- [ ] The `_in` wrappers (`contract::pay`, `payment::pay`, `payment::pay_bulk`,
      `issue::issue`): used by tests and the Discord commands, each commits once.
- [ ] The purge's five tables (`vc_core::purge`): a row that expires and one that
      does not, per table, in `tests/purge.rs`.

## 2. Dynamic

### 2.1 The gates and the suites

- [ ] `just check` — formatting, clippy (warnings denied), the whole suite, the
      sqlx offline data, the schema baseline. Any failure is a finding.
- [ ] The focused suites when one area moved:
      `cargo nextest run -p vc-api --test contract_idempotency --test contract_party
      --test contract_payments --test contract_pagination --test contract_escrow
      --test interactions_contract --test webhook_reverify`.
- [ ] `just sqlx-check`; after any new query, `just sqlx-prepare` — **never** a bare
      `cargo sqlx prepare`, which under mbx writes nothing and empties `.sqlx`
      (`scripts/sqlx-prepare.sh` says why). Check `git status .sqlx` immediately
      afterwards.

### 2.2 Prove the tests bite

Remove a mechanism, watch its test fail. This is what turns "reachable" from a
belief into a fact, and each of these was run while the code was written:

| Remove | Expect |
| --- | --- |
| `begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")` → `begin()` | `a_stricter_session_default_does_not_change_the_answer` answers 500 |
| `register_in` before `commit` | a replay loses its body |
| the `lock_timeout` around the claim | `a_key_another_request_is_holding_answers_come_back` hangs |
| `party_discord_id`'s filter in the draw | `a_charge_draws_on_the_party_it_names` draws from the wrong party |
| `PER_PAGE`'s `limited_to` on one list | `an_absent_limit_is_a_page` sees every row |
| `Page::next_cursor`'s "exactly full" test | a short page gains a `link` |
| the `assets` trigger | `contract_escrow`'s supply assertions fail |
| the signature check in `notification::verify` | the handshake tests pass an application that verifies nothing |

### 2.3 The database, by experiment

- [ ] **The default isolation**: `psql … -Atc "show default_transaction_isolation"`
      — `read committed`, and the idempotency transaction names it anyway.
- [ ] **Concurrency, at both levels**: two `psql` processes, one key, the second
      committing while the first holds the row. At `read committed` the second waits
      and then reads the row; at `repeatable read` it is refused with `40001`. The
      suite covers both outcomes (`concurrent_charges_with_one_key_write_once`,
      `a_stricter_session_default_does_not_change_the_answer`); the raw experiment
      is what says the suite is not a coincidence.
- [ ] **Supply and escrow under the API**: lock → charge → return → settle with data
      in the database, checking `SUM(assets.amount)`, the contract's account and
      `currency_payment_histories` after each step.
- [ ] **Upgrade shapes**: a database carrying a row that answers nothing (the
      pre-`4435878` shape) answers `409 processing` and charges nothing; a database
      migrated from the Ecto baseline passes `just baseline-check`.

### 2.4 Boundaries, with the answer each should get

| Input | Endpoint | Expected |
| --- | --- | --- |
| no `limit` | contract lists, statement | 200, fifty rows, `link` when full |
| no `limit` | claim list | 200, **every matching claim** — see §4 |
| `limit=0` | any list | 200, no rows, no `link` |
| `limit=-1`, `limit=201`, `limit=1000000` | any list | 400 `invalid_limit` |
| `limit=nyan` | any list | 400 `invalid_limit` |
| `next=abc`, `on_next=abc` | claims, contracts, statement | 400 `invalid_cursor` |
| `next=1&on_next=1` | the same | 400 `invalid_cursor` |
| `order=nonsense` | claim list | 400 `invalid_order` |
| a key that is unquoted, or 257 characters | any write | 400 `invalid_idempotency_key` |
| two `Idempotency-Key` headers | any write | 400 `multiple_idempotency_key_header_is_not_supported`, no `Idempotency-Status` |
| the same key twice | any write | the first answer, `Idempotency-Status: Duplicate` |
| a failure after the key is claimed | any write | 500 with no `Idempotency-Status`, and the key is free afterwards |
| a claim held for more than a second | any write | 409 `processing` (`should_retry_after_in_seconds`) |
| `amount` of `"0"`, `"-1"` | payment | 400 `invalid_amount` (the core's refusal) |
| `amount` of `"ten"` | payment | 400 — but `invalid_format_of_convert_amount`, see §4 |
| `amount` of `"ten"` | charge, issue | 400 `invalid_format_of_amount` |
| everything the contract holds plus one | charge | 409 `not_enough_amount` |
| `expires_in` of `0`, or of a year and a day | contract create | 400 `invalid_expires_in` |
| `party_discord_id` naming a stranger | charge | 400 `not_a_party` |
| `party_discord_id` naming a party whose remainder is short | charge | 409 `not_enough_amount`, even when the contract holds more |
| a fixed receiver, and a receiver that is neither it nor the party drawn on | charge | 400 `receiver_is_fixed` |
| a charge after the deadline | charge | 409 `expired` |
| `Accept` of `text/html` alone | any `/api` route | 406, before any handler |
| three handshakes within three seconds | a webhook | the third is `retry_after_3_seconds` |
| `RATE_LIMIT_PER_MINUTE=1`, two requests | any v2 route | 429 on the second |

### 2.5 Runtime shapes

- [ ] **The clock**: `VCRYPTO_SETTLE_INTERVAL_SECS=0` stops settling, purging and
      the webhook re-check together; with a short interval, an aged `expires_at`
      refunds within a tick and the application is notified.
- [ ] **The deploy build**: `SQLX_OFFLINE=true cargo build --release`.
- [ ] **Required env vars absent**: `vc-server/src/main.rs` refuses to start, with a
      clear message. Optional ones absent: the documented defaults.
- [ ] **The site**: `GET /` answers the index, a hashed asset may be kept forever,
      an unknown client route answers the index, and `/api/…` keeps its own routes.
- [ ] **The demo without Discord**: `demo-billing` with a missing argument exits 2;
      with no server it fails with a message and exits 1.

## 3. Human

Nothing here can be settled from a terminal.

- [ ] **`/help` and the site's command list** against what the commands do.
- [ ] **`/contract list`**: five rows, the count in the first line, the arrows
      (⏪ ⏮️ ⏭️ ⏩) and their disabled states, the four states a caller's own part can
      be in, the deadline line, and that a decision draws the first page again.
- [ ] **`/claim`**: every subcommand, the buttons, the page row (⏪ ⏮️ ⏭️ ⏩ 🔄), the
      selection menu, the metadata modal, the autocomplete lists.
- [ ] **`/grant` and `/issue`**: the ask, the `user_code`, the approval, and the
      refusal a guild that has not granted gets.
- [ ] **`/application register`**: the client id and secret, and the handshake that
      decides the registration.
- [ ] **A claim end to end**: created by an application, approved and denied, the
      webhook arriving both times, the money moving exactly once.
- [ ] **A metered run**: register an application, `/issue` a balance, run
      `demo-billing`, approve with the button, watch the charges fall to zero, and
      read the statement back — the same total.
- [ ] **A webhook, for real**: a public HTTPS endpoint that verifies signatures; the
      PING handshake at registration; the type-4 event when a contract goes `active`;
      a refusal, a withdrawal and an expiry arriving as `canceled`/`expired`.
- [ ] **The re-check against that webhook**: `/oauth2/clients/@me` after the job has
      run; then make the endpoint answer 200 to the false-signature PING (or stop
      answering) and confirm `webhook_failed_at` moves and deliveries continue.
- [ ] **A deployment**: Fly, the worker proxy and its certificate, a delivery
      through it, the scheduler as a task rather than in a request.
- [ ] **The clock over a real day**: a contract settled by the tick, a pool refill
      across a UTC day boundary (`job_runs`).
- [ ] **The Elixir beside it**: point it at the same database after `0009`/`0010` and
      confirm it neither reads nor writes the columns it does not know.

## 4. What this document cannot settle

Each of these is a decision rather than a defect; a reviewer should give a
recommendation and what it costs:

- [ ] **The claim list answers every matching claim when no `limit` is given**,
      where the contract lists and the statement answer fifty. It is a ported
      endpoint whose documented behaviour is "no limit means no limit", and it is
      now the only unbounded-by-default list in the service. Align it (a deviation
      to record) or say in `docs/known-gaps.md` why not.
- [ ] **Is fifty the right page**, with a ceiling of two hundred? The size is a
      judgement; the bounds are tested.
- [ ] **Is a second the right wait** for a key another request holds
      (`CLAIM_WAIT`)? It decides whether a retry waits for the answer or is told to
      come back.
- [ ] **Is `409 processing` with `should_retry_after_in_seconds` the right thing to
      tell a client**, and is the `Idempotency-Status` header worth what it costs?
- [ ] **Is `invalid_format_of_convert_amount` the right name** for a non-numeric
      `amount` in the single-payment clause of `POST /api/v2/users/@me/transactions`
      (`routes/v2/transactions.rs:86`)? It is what the Elixir said for that clause,
      and the charge and issue endpoints call the same mistake
      `invalid_format_of_amount`. Either it is fidelity worth keeping or a name to
      fix in one line.
- [ ] **The two screens count differently on purpose**: the contract screen says a
      number (a `COUNT`), the claim screen fetches one row more than a page holds and
      says "and more". Both are legitimate; confirm both read well.
- [ ] **Whether a deployment ever sets an isolation level above `READ COMMITTED`**:
      the idempotency transaction names its own, so the answer is the same either
      way, but the session default is what a `psql` session inherits.
- [ ] **The Japanese copy** on the Discord screens and the site.

## Appendix A: what changed recently

The newest commits are the surface most worth a reviewer's time, and each message
says what the mechanism was. Newest first:

| Commit | What it is |
| --- | --- |
| `5b6071e` | the contract screen pages: arrows, a count, and both pagination mechanisms kept |
| `3af70bf` | the ceiling on `limit`; the contract screen's count and page (it used to read every contract the caller is named in) |
| `4282238` | this document, first version |
| `fb41dc3` | every contract list answers a page (the two older ones answered every row) |
| `a6aa784` | the two key refusals nothing pinned (several keys; a key over 256 characters) |
| `cff3380` | the idempotency transaction names its isolation level; the `40001` branch it made unreachable is gone |
| `a81e94b`, `4435878`, `8a71980` | the claim, the write and the answer are one commit; the wait for another request's claim is bounded |
| `153098d`, `5dfd892`, `73366c8` | a client's typo is a 400 (limit, cursor, order); a key is spent by a write and only by a write |

## Appendix B: where the project keeps its notes

| File | What it holds |
| --- | --- |
| `docs/known-gaps.md` | what is deliberately missing or different, and why |
| `docs/test-port.md` | every Elixir test case, ported or dropped |
| `docs/contracts.md`, `docs/issue.md`, `docs/oauth2.md` | the designed contracts: money, applications, guild tokens |
| `docs/deploy.md` | the environment a deployment needs |
| `docs/web-ui.md` | what the frontend replaced, and what the service answers |
| `docs/qa.md` | this document |
