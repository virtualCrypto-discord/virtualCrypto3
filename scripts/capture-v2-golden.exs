# Captures golden HTTP responses for the v2 endpoints that have no test
# coverage in the Elixir suite: GET /api/v2/users/@me and
# GET /api/v2/users/@me/balances.
#
# The fixture is fully deterministic (fixed guild/user/currency identifiers) so
# the goldens are stable. Run with:
#
#   MIX_ENV=test mix run capture_v2_golden.exs
#
# NOTE: Discord.Api.OAuth2.get_user_info/1 is patched in this clone to return a
# fixed payload; /users/@me otherwise performs a live Discord API call.

alias VirtualCrypto.Repo
alias VirtualCrypto.Exterior.User.Discord, as: DiscordUser

Ecto.Adapters.SQL.Sandbox.mode(Repo, :auto)

out_dir = Path.expand("/path/to/virtualCrypto3/crates/vc-api/tests/golden")
File.mkdir_p!(out_dir)

user1_discord = 100_000_000_000_000_001
user2_discord = 100_000_000_000_000_002
guild = 900_000_000_000_000_001
guild2 = 900_000_000_000_000_002

# --- deterministic fixture -------------------------------------------------
{:ok} =
  VirtualCrypto.Money.create(
    guild: guild,
    name: "nyan",
    unit: "nyan",
    creator: %DiscordUser{id: user1_discord},
    creator_amount: 200_000
  )

{:ok} =
  VirtualCrypto.Money.create(
    guild: guild2,
    name: "wan",
    unit: "wan",
    creator: %DiscordUser{id: user2_discord},
    creator_amount: 200_000
  )

{:ok} =
  VirtualCrypto.Money.pay(
    sender: %DiscordUser{id: user1_discord},
    receiver: %DiscordUser{id: user2_discord},
    amount: 500,
    unit: "nyan"
  )

{:ok, _} = VirtualCrypto.Money.give(receiver: %DiscordUser{id: user2_discord}, amount: 500, guild: guild)

{:ok, u1} = VirtualCrypto.User.insert_user_if_not_exists(user1_discord)
{:ok, u2} = VirtualCrypto.User.insert_user_if_not_exists(user2_discord)

{:ok, _} =
  VirtualCrypto.DiscordAuth.insert_user(
    user1_discord,
    "stub-token",
    NaiveDateTime.add(NaiveDateTime.utc_now(), 3600),
    "stub-refresh-token"
  )

scopes = ["oauth2.register", "vc.pay", "vc.claim"]
{:ok, token1, _} = VirtualCrypto.Guardian.issue_token_for_user(u1.id, scopes)
{:ok, token2, _} = VirtualCrypto.Guardian.issue_token_for_user(u2.id, scopes)

write = fn name, resp ->
  body =
    case Jason.decode(resp.resp_body) do
      {:ok, json} -> json
      _ -> resp.resp_body
    end

  payload = %{
    "request" => %{"method" => resp.method, "path" => resp.request_path},
    "status" => resp.status,
    "headers" => Map.new(resp.resp_headers),
    "body" => body
  }

  File.write!(Path.join(out_dir, name), Jason.encode!(payload, pretty: true))
  IO.puts("#{name}: #{resp.status}")
end

base_conn = fn ->
  Phoenix.ConnTest.build_conn() |> Plug.Conn.put_req_header("accept", "application/json")
end

dispatch = fn conn, path ->
  Phoenix.ConnTest.dispatch(conn, VirtualCryptoWeb.Endpoint, :get, path)
end

auth = fn conn, token -> Plug.Conn.put_req_header(conn, "authorization", "Bearer " <> token) end

# --- GET /api/v2/users/@me -------------------------------------------------
write.(
  "v2_users_me.json",
  dispatch.(auth.(base_conn.(), token1), "/api/v2/users/@me")
)

write.(
  "v2_users_me_no_auth.json",
  dispatch.(base_conn.(), "/api/v2/users/@me")
)

write.(
  "v2_users_me_garbage_token.json",
  dispatch.(auth.(base_conn.(), "not-a-jwt"), "/api/v2/users/@me")
)

# a correctly signed token whose jti has been revoked (row deleted)
{:ok, revoked, _} = VirtualCrypto.Guardian.issue_token_for_user(u1.id, scopes)
{:ok, revoked_claims} = VirtualCrypto.Guardian.decode_and_verify(revoked)
VirtualCrypto.Guardian.revoke_with_jti(revoked_claims)

write.(
  "v2_users_me_revoked_token.json",
  dispatch.(auth.(base_conn.(), revoked), "/api/v2/users/@me")
)

# --- GET /api/v2/users/@me/balances ---------------------------------------
write.(
  "v2_users_me_balances.json",
  dispatch.(auth.(base_conn.(), token1), "/api/v2/users/@me/balances")
)

write.(
  "v2_users_me_balances_user2.json",
  dispatch.(auth.(base_conn.(), token2), "/api/v2/users/@me/balances")
)

write.(
  "v2_users_me_balances_no_auth.json",
  dispatch.(base_conn.(), "/api/v2/users/@me/balances")
)

write.(
  "v2_users_me_balances_garbage_token.json",
  dispatch.(auth.(base_conn.(), "not-a-jwt"), "/api/v2/users/@me/balances")
)

IO.puts("goldens written to #{out_dir}")
