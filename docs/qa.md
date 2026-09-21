# QA, split by who can decide it

Three kinds of verification, and the split is by *who can settle the question*:

- **Static** — a reader, human or model, comparing one artefact against another:
  code against docs, a claim against its evidence, a test against the thing it says
  it pins. No execution and no environment.
- **Dynamic** — a shell and a database: the gates, the focused suites, and the
  experiments that need rows and concurrent connections.
- **Human** — Discord, a real webhook, a real deployment, and judgement about
  whether a behaviour is the right one.

Each item says what to look at and what "pass" is. A finding goes where the project
already keeps its notes: an accepted difference in `docs/known-gaps.md`, a bug as a
failing test first, and the reasoning in the commit message.

Two findings from writing this file, kept here so the list is not read as
speculation: **`POST /oauth2/token/revoke` has no test** (the helpers it calls do),
and **`PATCH /oauth2/clients/@me` has no test through HTTP** (only
`vc_core::application::patch` is tested). Both are reachable by a client.

## Static

### The surface, endpoint by endpoint

Every route, who may call it, and the suite that would notice if it broke. The
column that matters is the last one: an endpoint whose suite is "—" is either
covered somewhere indirect or is a finding.

| Endpoint | Caller | Suite |
| --- | --- | --- |
| `GET /health` | — | `web` (a smoke test) |
| `GET /login`, `GET /logout`, `GET /callback/discord`, `GET /invite`, `GET /support`, `POST /token` | session | `login`, `oauth2_token_endpoint` |
| `GET /assets` | — | `web` |
| `POST /api/integrations/discord/interactions` | Ed25519 signature | `interactions_*` (ten files), `discord_schema` |
| `GET /oauth2/authorize`, `POST /oauth2/authorize` | session | `oauth2_authorize`, `oauth2_preauthorize` |
| `POST /oauth2/token` | client credentials (Basic) | `oauth2_token`, `oauth2_token_endpoint`, `grant_requests` |
| `POST /oauth2/token/revoke` | — | **—** |
| `GET /oauth2/clients` | app token | `oauth2_clients_mine` |
| `POST /oauth2/clients` | user token | `oauth2_clients` |
| `GET /oauth2/clients/@me` | app token | `oauth2_clients_me`, `webhook_reverify` |
| `PATCH /oauth2/clients/@me` | app token | **—** (the core function has tests) |
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

- [ ] **Walk the table against `crates/vc-api/src/routes/mod.rs` and
      `routes/v2/mod.rs`**: every path there is a row, and no row has been dropped.
- [ ] **Close the two gaps named above**, or record why they stay: revocation is
      the one endpoint a *client* calls to undo a leak, and the edit is the only way
      an application changes its own webhook.
- [ ] **For each endpoint, the auth column against the extractor**: `AuthUser`
      vs `Limited` vs `GuildToken`, the `kind` check inside, and the scope. A v2
      handler that takes `AuthUser` instead of `Limited` is an unlimited one;
      `routes/limited.rs` is the list of what is limited.
- [ ] **Unknown fields are ignored, and adding response fields is not breaking**
      (`docs/known-gaps.md` quotes the specification). Check that no handler
      refuses a body for carrying a field it does not know.

### The Discord surface

Twelve commands — `help`, `invite`, `application`, `issue`, `grant`, `contract`,
`pay`, `info`, `create`, `delete`, `bal`, `claim` — and each is four things that
have to agree:

- [ ] **Registration ↔ handler ↔ prose ↔ suite.** `discord_commands.rs` (the
      payload, options, `contexts`, `integration_types`) ↔ `command/mod.rs`'s
      `handle` arm ↔ `docs/commands.rs` (the prose shown by `/help` and on the
      site) ↔ `tests/interactions_<name>.rs`. `tests/documentation.rs` asserts the
      first, third and fourth exist and are in order; a reader checks the second
      and that the prose describes what the handler does.
- [ ] **The state a component carries.** Every button, select and modal packs its
      state into a `custom_id` (`custom_id.rs`): a reader confirms the head byte,
      the action, the id, and that a stale id is answered rather than trusted.
- [ ] **The screens nobody clicks in a test**: the "ほかK件" line, the error
      screens (`CommandError`), the ephemeral flags, the autocomplete lists.
- [ ] **`contexts` and `integration_types`** per command: `/contract list` runs in
      a DM as well as a guild; the admin-only commands carry the default
      permissions bit.

### The joins between artefacts

- [ ] **Every documented endpoint is a route and every route is documented** —
      `tests/documentation.rs` asserts the join's existence; a reader checks the
      *content* (`docs/api.rs` against the handlers), including recently added
      notes (`limit` defaults, `party_discord_id`, `Idempotency-Key`).
- [ ] **`docs/contracts.md` against the contract handlers**: the three rules under
      "Retrying a charge" are `routes/idempotency.rs`'s `guard`; the return rule is
      the `returning` check in `vc_core::contract::pay_in`; the statement's grain is
      the `contract_id` write and `contract::payments`; the page default is
      `PER_PAGE`.
- [ ] **`docs/known-gaps.md` against the tree**: every claim in it is a file, a
      function or a test. Walk each, especially the recent ones (a client's typo is
      a 400; the claim is in the write's transaction; the level is named).
- [ ] **`docs/issue.md`, `docs/oauth2.md`, `docs/deploy.md`, `docs/test-port.md`**
      against their subjects: the issue scope and the device poll, the OAuth2
      metadata rules and the rendered application, the env vars, and the port
      ledger's "dropped" column.
- [ ] **`docs/web-ui.md`'s "what the service answers"** against the SPA in `web/`:
      every page it fetches is a route.
- [ ] **`README.md`, `justfile`, `flake.nix`, `scripts/`** against each other: the
      mbx note against `sqlx-prepare.sh`, the gates in `just check` against CI's
      `.github/workflows/ci.yml`, `docs/deploy.md`'s env vars against
      `vc-server/src/main.rs`.
- [ ] **Each commit's message against its diff**, newest first: the messages make
      specific claims and a reader confirms the diff is that and nothing else.
- [ ] **The migrations' comments against their SQL**: `0009` says the rows already
      written are not backfilled (they cannot be) and the migration does exactly
      that.

### Every branch, and what makes it reachable

An unreachable branch nothing names is a lie about the code; a reachable one
nothing tests is a hole. Start with the idempotency layer:

| Branch | Reachable | What makes it so | Pinned by |
| --- | --- | --- | --- |
| the key is claimed | yes | an ordinary first request | `a_repeated_key_charges_once`, `concurrent_charges_with_one_key_write_once` |
| the key has an answer | yes | a retry | the same two (`Duplicate`, same body) |
| the write failed | yes | a database failure (the table is dropped in the test) | `a_failure_gives_the_key_back` |
| the wait ran out | yes | another transaction holds the row for a second | `a_key_another_request_is_holding_answers_come_back` |
| a row that answers nothing | yes, *as data* | rows claimed by the version before `4435878` | `a_row_that_answers_nothing_asks_the_caller_to_come_back` |
| refused by the isolation level | **no** | — the transaction names `READ COMMITTED` | `a_stricter_session_default_does_not_change_the_answer` |
| a key that is unquoted, too long, or several | yes | a client's mistake | `an_unquoted_key_is_rejected`, `an_over_long_key_is_refused`, `two_keys_at_once_are_refused` |

- [ ] **Two mechanisms, both kept**: the API pages with a cursor (`next`/`on_next`
      through `Page::asked`, `vc_core::page::Cursor`) and the Discord screens page
      with numbers (`claim::list_page`, `contract::open_of_party`). A reader
      confirms neither has quietly replaced the other, and that each is used where
      it is the right one.
- [ ] **Re-derive the table** from `routes/idempotency.rs` and
      `vc_core/idempotency.rs`: every `match` arm is in it, and the unreachable one
      has a reason a reader can check rather than "should not happen".
- [ ] **The same pass over the rest**: `contract::pay_in`'s error arms,
      `Page::asked`'s bounds, `scheduler::reverify_webhooks`'s handshake outcomes,
      `notification::check_webhook`'s transport choice, `interactions.rs`'s type
      dispatch, the OAuth2 token endpoint's four grants.
- [ ] **No defensive branch without a name**: anything shaped like
      `unwrap_or_default`, a catch-all `_ =>` arm or a "just in case" check — is the
      case reachable, and does the comment say by what?

### Invariants

- [ ] **"The escrow is the parties' remainders."** Read `contract::approve`
      (debit the party, credit the contract's account), `pay_in` (debit the
      contract, credit the receiver) and `end` (refund): no path writes `remaining`
      without moving `assets` in the same transaction.
- [ ] **"A lock is a transfer, not a deletion."** `tests/contract_escrow.rs`'s
      `supply()` and `escrow()` state it; the pool arithmetic
      (`currency::reset_pool_amount`) is the reader that would break otherwise.
- [ ] **A claim's money**: refuse and withdraw return every locked remainder;
      expiry settles the same way; a spend after the deadline is refused whether or
      not the tick has run.
- [ ] **Idempotency**: one key, one write — success, refusal and failure each leave
      the key in the state the three rules in `docs/contracts.md` describe.
- [ ] **Pagination**: a page is bounded, the cursor is the ordered column, and a
      short page is the end (no `link`). `Page::next_cursor` is the only place that
      decides "there is more".
- [ ] **Error shapes**: `ApiError::parts()` against the hand-written bodies in
      `payment_error` / `issue_error` / `contract_error` — same status, same keys,
      same words where they overlap.
- [ ] **Every `_in` write is used inside a transaction the layer owns**:
      `grep -n "begin()" crates/vc-core/src/{contract,payment,issue}.rs` — the
      wrappers open one, the `_in` functions never do.

### Ported fidelity, and where it was deliberately dropped

- [ ] **The goldens**: `tests/golden/` (fourteen files) are captured from the
      Elixir; `assert_matches_golden` compares status, body and content type
      exactly. A reader confirms no fixture has been edited to fit the code.
- [ ] **`docs/test-port.md`'s ledger**: every Elixir test case is a file here or is
      recorded as dropped, and each "dropped" has a reason.
- [ ] **`docs/known-gaps.md`'s deliberate differences**, one by one, against the
      code: the negative limit, the cursor and the order (400 rather than the
      Elixir's 500), `/issue` rather than `/give`, 303 rather than 302, the consent
      screen's repairs, `POST /oauth2/token`'s 400, the member's unknown role id.
- [ ] **Vacuous assertions**: a test that would pass if the subject were broken —
      the shape to look for is an id compared against the wrong account (one
      existed here: a key counted for an application's row id instead of its
      account id, always zero), or a loop that never runs.

### Coverage: what nothing tests

- [ ] **The two named at the top** (`/oauth2/token/revoke` through HTTP,
      `PATCH /oauth2/clients/@me` through HTTP).
- [ ] **The `_in` functions' wrappers**: `contract::pay`, `payment::pay`,
      `payment::pay_bulk`, `issue::issue` — used by tests and the Discord
      commands; a reader confirms each still commits exactly once.
- [ ] **`/health`**: what it answers and what reads it (a deployment's check).
- [ ] **The purge's five tables** (`vc_core::purge`): each has a row in
      `tests/purge.rs` that expires and a row that does not.
- [ ] **The Discord commands' rare arms**: a command name that is not registered,
      a component with a forged `custom_id`, a modal submitted empty.

## Dynamic

### The gates

- [ ] `just check` — formatting, clippy with warnings denied, the whole suite, the
      sqlx data, the baseline. Any failure is a finding.
- [ ] `just test` alone, and the focused suites when one area moved:
      `cargo nextest run -p vc-api --test contract_idempotency --test contract_party
      --test contract_payments --test contract_pagination --test contract_escrow
      --test webhook_reverify --test interactions_contract`.
- [ ] `just sqlx-check`, and after any new query `just sqlx-prepare` — *not* a bare
      `cargo sqlx prepare`, which under mbx writes nothing and empties `.sqlx`.
      Check `git status .sqlx` immediately afterwards.

### Prove the tests bite

Remove a mechanism, watch its test fail. This is what turns "reachable" from a
belief into a fact, and it is how two of the claims above were checked:

- [ ] `begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")` → `begin()`: the
      isolation test answers 500.
- [ ] `register_in` before `commit`: a replay loses its body.
- [ ] the `lock_timeout` around the claim: `a_key_another_request_is_holding_…`
      hangs rather than answering.
- [ ] `party_discord_id`'s filter in the draw: `a_charge_draws_on_the_party_it_names`
      draws from the wrong party.
- [ ] the `assets` trigger (delete a balance that reaches zero): the supply
      assertions in `contract_escrow` fail.
- [ ] `Page::next_cursor`'s "exactly full" test: a short page gains a `link`.
- [ ] the webhook signature check in `notification::verify`: the handshake tests
      pass an application that verifies nothing.

### The database, by experiment

- [ ] **The default isolation**: `psql … -Atc "show
      default_transaction_isolation"` — `read committed`, which is what the claim's
      behaviour is reasoned about at (and the transaction says so anyway).
- [ ] **Concurrency at the right level**: two `psql` processes, one key, the second
      committing while the first holds the row (`INSERT … ON CONFLICT DO NOTHING
      RETURNING id`). At `read committed` the second waits and then sees the row;
      at `repeatable read` it is refused with `40001`. The suite covers both
      outcomes; the raw experiment is what says the suite is not a coincidence.
- [ ] **Supply and escrow under the API**: run lock → charge → return → settle with
      data in the database and check `SUM(assets.amount)`, the contract's account
      and `currency_payment_histories` after each step.
- [ ] **Upgrade shapes**: a database carrying a row that answers nothing (the
      pre-`4435878` shape) answers `409 processing` and charges nothing; a database
      migrated from the Ecto baseline passes `just baseline-check`.

### Boundaries through the API

- [ ] A table of inputs, each against the endpoints that read it: `limit` of
      `-1`, `0`, `1`, `1000000`; `next`/`on_next` of a number, of text, of both;
      `order` of the two known values and of anything else; a key that is unquoted,
      257 characters, or two headers; an `amount` of `"0"`, `"-1"`, `"ten"`, and
      everything the contract holds plus one; `expires_in` of `0`, of a year, of a
      year and a day; `party_discord_id` of a party, of a stranger, of the receiver;
      `unit` of a currency that does not exist; a body that is `null`, `[]` or
      `"string"`.
- [ ] **Content negotiation**: no `Accept`, `Accept: */*`, `Accept:
      application/json` and `Accept: text/html` on one v2 endpoint and one site
      route.
- [ ] **Rate limiting**: `RATE_LIMIT_PER_MINUTE=1` and two requests → 429 on the
      second; `0` disables it. The verification limiter: three handshakes in three
      seconds → `retry_after_3_seconds`.

### Runtime shapes

- [ ] **The clock**: `VCRYPTO_SETTLE_INTERVAL_SECS=0` stops settling, purging and
      the webhook re-check together; with a short interval, an aged `expires_at`
      refunds within a tick and the application is notified.
- [ ] **The deploy build**: `SQLX_OFFLINE=true cargo build --release` — the compile
      a deployment does, against the committed data.
- [ ] **The server's env vars** (`vc-server/src/main.rs`): required ones absent →
      a clear refusal to start; optional ones absent → the documented defaults.
- [ ] **The site**: `GET /` serves the index, a hashed asset is cached forever, an
      unknown client route answers the index, and `/api/…` keeps its own routes
      (`tests/web.rs`).
- [ ] **The demo's edges without Discord**: `demo-billing` with a missing argument
      exits 2; with no server it fails with a message and exits 1.

## Human

Nothing here can be concluded from a terminal.

### Discord

- [ ] **`/help` and the site's command list** against what the commands actually
      do — the copy is generated from the registration, so a wrong description is a
      wrong registration.
- [ ] **`/contract list`**: the screen, the five-contract cap and the "ほかK件"
      line, the four states a caller's own part can be in, the deadline line, the
      buttons offered in each state, and that answering one advances the list.
- [ ] **`/claim`**: every subcommand, the buttons, the metadata modal, the
      autocomplete lists, and the pagination if the list has any.
- [ ] **`/grant` and `/issue`**: the ask, the `user_code`, the approval, and the
      refusal a guild that has not granted gets.
- [ ] **`/application register`**: the client id and secret it answers, and that
      the webhook handshake it runs is the one that decides.
- [ ] **`/bal`, `/info`, `/pay`, `/create`, `/delete`, `help`, `invite`**: the
      screens, in Japanese, as a person reads them.

### End to end, as a person

- [ ] **A claim**: created by an application, approved and denied by the parties,
      the webhook event arriving both times, the money moving exactly once.
- [ ] **A metered run**: register an application, `/issue` a balance, run
      `demo-billing`, approve with the button, watch the charges fall to zero, and
      read the statement back — the same total.
- [ ] **A webhook, for real**: a public HTTPS endpoint that verifies signatures,
      the PING handshake at registration, the type-4 event when a contract goes
      `active`, and a decision (refusal, withdrawal, expiry) as `canceled`/`expired`.
- [ ] **The re-check against that webhook**: `/oauth2/clients/@me` after the job has
      run — `webhook_verified_at` set; then make the endpoint answer 200 to the
      false-signature PING (or stop answering) and confirm `webhook_failed_at` moves
      and deliveries continue.
- [ ] **A deployment**: Fly, the worker proxy and its certificate, a delivery
      through it, and the scheduler running as a task rather than in a request.
- [ ] **The clock over a real day**: a contract settled by the tick, and a pool
      refill across a UTC day boundary (`job_runs`).
- [ ] **The Elixir beside it**: point the Elixir service at the same database after
      `0009`/`0010` and confirm it neither reads nor writes the columns it does not
      know, and that its own flows still work.

### Judgement, which is the part no test has

- [ ] Is a second's bounded wait the right answer for a retry (`CLAIM_WAIT`)? Is
      `409 processing` with `should_retry_after_in_seconds` the right thing to tell
      a client?
- [ ] Is a page of fifty, with a ceiling of two hundred, right for the lists and
      the statement? (The size is the judgement; the ceiling and the screens' pages
      are done.) Two screens answer "how many are there" differently — the contract
      screen counts (`open_of_party` says the total and the four pages around it),
      the claim screen fetches one row more than a page holds and says "and more" —
      and a person should confirm both read well.
- [ ] Does a request that fails leave the caller able to retry *the same* request?
      (`docs/contracts.md`'s three rules are the answer; a person checks the answer
      is the right one.)
- [ ] Is the Japanese copy on the Discord screens and the site what we want to say?
- [ ] **A fresh reader**: give `docs/` to somebody who was not in this session and
      ask what they think the service does. Every hesitation is a doc that assumes
      what only the author knew.

## Reporting a finding

- A **bug**: a failing test first, then the fix, then the message saying what the
  mechanism was and why the old shape looked right.
- An **accepted difference** from the Elixir: a paragraph in `docs/known-gaps.md`
  that says what changed and what decided it.
- A **behaviour that is right but surprising**: a line in `docs/contracts.md` or
  `docs/issue.md` where a reader will look, and a test that pins it.
- A **coverage gap**: a test, or a line here saying why there is not one.
