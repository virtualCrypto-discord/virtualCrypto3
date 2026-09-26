#!/usr/bin/env bash
# Source this to run the service against the Discord test application:
#
#   . scripts/discord-env.sh
#
# The bot token is `VCRYPTO_BOT_TOKEN`, the name the service reads; the application's
# id and Ed25519 public key are then read back from Discord, so they can never drift
# from whichever token was exported. Anything already exported wins.
set -euo pipefail

# The names the service reads; `docs/deploy.md` has the full list.
: "${VCRYPTO_BOT_TOKEN:?VCRYPTO_BOT_TOKEN (a bot token) must be set in the environment}"

application="$(
  curl -sS -H "Authorization: Bot $VCRYPTO_BOT_TOKEN" \
    https://discord.com/api/v10/applications/@me
)"

if [[ "$(jq -r '.message // empty' <<<"$application")" != "" ]]; then
  echo "Discord rejected the token: $(jq -r '.message' <<<"$application")" >&2
  return 1 2>/dev/null || exit 1
fi

export VCRYPTO_CLIENT_ID="${VCRYPTO_CLIENT_ID:-$(jq -r '.id' <<<"$application")}"
export VCRYPTO_PUBLIC_KEY="${VCRYPTO_PUBLIC_KEY:-$(jq -r '.verify_key' <<<"$application")}"

# Local defaults; the Elixir test config's values stand in for the OAuth paths,
# which the interaction endpoint does not use.
export VCRYPTO_ENV="${VCRYPTO_ENV:-development}"
export DATABASE_URL="${DATABASE_URL:-postgres://postgres:postgres@localhost:5432/virtualcrypto_dev}"
export VCRYPTO_API_JWT_SECRET_KEY="${VCRYPTO_API_JWT_SECRET_KEY:-a188rolUOVnGqP7wseWeTW0qkFCfsDMNvbo2Bz6O3dmO9TEyKPD8+Yf1bfiUFRBI}"
export VCRYPTO_CLIENT_SECRET="${VCRYPTO_CLIENT_SECRET:-test-client-secret}"
export PORT="${PORT:-8080}"
export VCRYPTO_SITE_URL="${VCRYPTO_SITE_URL:-http://localhost:${PORT}}"
export VCRYPTO_INVITE_URL="${VCRYPTO_INVITE_URL:-https://discord.com/api/oauth2/authorize?client_id=${VCRYPTO_CLIENT_ID}&permissions=0&scope=applications.commands%20bot}"

# HTTP development uses temporary OAuth binding cookies without Secure.
export SECURE_COOKIES="${SECURE_COOKIES:-false}"

echo "discord app: $VCRYPTO_CLIENT_ID ($(jq -r '.name' <<<"$application"))"
echo "public key:  $VCRYPTO_PUBLIC_KEY"
echo "in guilds:   $(jq -r '.approximate_guild_count // 0' <<<"$application")"
echo "listening:   http://localhost:${PORT}"
