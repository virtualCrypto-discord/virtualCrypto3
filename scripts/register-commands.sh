#!/usr/bin/env bash
# Register the application commands with Discord, replacing whatever is there
# (the same bulk overwrite the Elixir `priv/register-commands.exs` performs).
#
#   scripts/register-commands.sh             # global: up to an hour to appear
#   scripts/register-commands.sh <guild_id>  # a guild: immediate
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=./discord-env.sh
source "$repo_root/scripts/discord-env.sh" >/dev/null

if [[ $# -ge 1 ]]; then
  url="https://discord.com/api/v10/applications/${VCRYPTO_CLIENT_ID}/guilds/$1/commands"
  scope="commands in guild $1"
else
  url="https://discord.com/api/v10/applications/${VCRYPTO_CLIENT_ID}/commands"
  scope="global commands"
fi

response="$(
  curl -sS -X PUT "$url" \
    -H "Authorization: Bot $VCRYPTO_BOT_TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary "@$repo_root/scripts/discord-commands.json"
)"

if [[ "$(jq -r 'if type == "array" then "ok" else (.message // "unknown error") end' <<<"$response")" != "ok" ]]; then
  jq -r '.message // .' <<<"$response" >&2
  exit 1
fi

echo "registered $(jq 'length' <<<"$response") $scope"
