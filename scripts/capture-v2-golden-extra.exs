# Second capture pass: the `:accepts, ["json"]` content negotiation matrix and
# the Discord token refresh path, both of which the first pass did not cover.
#
# Run inside the upstream clone AFTER capture_v2_golden.exs (it reuses that
# fixture) and with both oauth2.ex patches applied:
#
#   MIX_ENV=test GOLDEN_DIR=/path/to/virtualCrypto3/crates/vc-api/tests/golden \
#     mix run capture_v2_golden_extra.exs

alias VirtualCrypto.Repo

Ecto.Adapters.SQL.Sandbox.mode(Repo, :auto)

out_dir = Path.expand(System.get_env("GOLDEN_DIR", "crates/vc-api/tests/golden"))
File.mkdir_p!(out_dir)

discord_id = 100_000_000_000_000_001
user = Repo.get_by!(VirtualCrypto.User.User, discord_id: discord_id)
{:ok, token, _} = VirtualCrypto.Guardian.issue_token_for_user(user.id, ["oauth2.register", "vc.pay", "vc.claim"])

write = fn name, resp, extra ->
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

  payload = if extra, do: Map.put(payload, "extra", extra), else: payload
  File.write!(Path.join(out_dir, name), Jason.encode!(payload, pretty: true))
  IO.puts("#{name}: #{resp.status}")
end

conn = fn accept ->
  base = Phoenix.ConnTest.build_conn()
  base = Plug.Conn.put_req_header(base, "authorization", "Bearer " <> token)
  if accept, do: Plug.Conn.put_req_header(base, "accept", accept), else: base
end

dispatch = fn c -> Phoenix.ConnTest.dispatch(c, VirtualCryptoWeb.Endpoint, :get, "/api/v2/users/@me") end

# --- content negotiation ---------------------------------------------------
# In test env Phoenix re-raises NotAcceptableError instead of rendering it, so
# the 406 response is rebuilt with the real ErrorJSON renderer and the
# exception's status. Status and body are therefore authoritative.
not_acceptable = fn ->
  try do
    dispatch.(conn.("text/html"))
  rescue
    e in Phoenix.NotAcceptableError ->
      body = VirtualCryptoWeb.ErrorJSON.render("406.json", %{})

      %{
        method: "GET",
        request_path: "/api/v2/users/@me",
        status: Plug.Exception.status(e),
        resp_headers: [
          {"cache-control", "max-age=0, private, must-revalidate"},
          {"content-type", "application/json; charset=utf-8"}
        ],
        resp_body: Jason.encode!(body)
      }
  end
end

write.("v2_users_me_accept_absent.json", dispatch.(conn.(nil)), nil)
write.("v2_users_me_accept_any.json", dispatch.(conn.("*/*")), nil)
write.("v2_users_me_accept_html.json", not_acceptable.(), nil)
write.("v2_users_me_accept_json_and_html.json", dispatch.(conn.("application/json, text/html")), nil)

# --- Discord token refresh -------------------------------------------------
# Move the stored authorization outside the seven-day window so refresh_user
# takes the refresh branch, then record the row it leaves behind.
#
# The column holds *UTC* naive timestamps written by Elixir, while now() returns
# the server's local time, so convert explicitly or the window check misses.
Repo.query!(
  "UPDATE discord_users
      SET updated_at = ((now() AT TIME ZONE 'utc') - interval '7 days')
    WHERE discord_user_id = $1",
  [discord_id]
)

iso = fn
  nil -> nil
  value -> NaiveDateTime.to_iso8601(value)
end

read_row = fn ->
  [row] =
    Repo.query!(
      "SELECT token, refresh_token, expires, updated_at
         FROM discord_users WHERE discord_user_id = $1",
      [discord_id]
    ).rows

  [token, refresh_token, expires, updated_at] = row

  %{
    "token" => token,
    "refresh_token" => refresh_token,
    "expires" => iso.(expires),
    "updated_at" => iso.(updated_at)
  }
end

before = read_row.()
response = dispatch.(conn.("application/json"))
after_ = read_row.()

IO.puts("refresh before: #{inspect(before)}")
IO.puts("refresh after : #{inspect(after_)}")

write.(
  "v2_users_me_refresh.json",
  response,
  %{"discord_users_before" => before, "discord_users_after" => after_}
)
