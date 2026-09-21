#!/usr/bin/env bash
# Source this to run the service against the Discord test application:
#
#   . scripts/discord-env.sh
#
# The bot token is read from `.env` (gitignored); the application's id and
# Ed25519 public key are then read back from Discord, so they can never drift
# from whichever token is in `.env`. Anything already exported wins.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -f "$repo_root/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$repo_root/.env"
  set +a
fi

: "${DISCORD_API_KEY:?DISCORD_API_KEY (a bot token) must be set in .env}"

# The names the service reads; `docs/deploy.md` has the full list.
export DISCORD_BOT_TOKEN="${DISCORD_BOT_TOKEN:-$DISCORD_API_KEY}"

application="$(
  curl -sS -H "Authorization: Bot $DISCORD_BOT_TOKEN" \
    https://discord.com/api/v10/applications/@me
)"

if [[ "$(jq -r '.message // empty' <<<"$application")" != "" ]]; then
  echo "Discord rejected the token: $(jq -r '.message' <<<"$application")" >&2
  return 1 2>/dev/null || exit 1
fi

export DISCORD_CLIENT_ID="${DISCORD_CLIENT_ID:-$(jq -r '.id' <<<"$application")}"
export DISCORD_PUBLIC_KEY="${DISCORD_PUBLIC_KEY:-$(jq -r '.verify_key' <<<"$application")}"

# Local defaults; the Elixir test config's values stand in for the OAuth paths,
# which the interaction endpoint does not use.
export DATABASE_URL="${DATABASE_URL:-postgres://postgres:postgres@localhost:5432/virtualcrypto_dev}"
export GUARDIAN_SECRET_KEY="${GUARDIAN_SECRET_KEY:-a188rolUOVnGqP7wseWeTW0qkFCfsDMNvbo2Bz6O3dmO9TEyKPD8+Yf1bfiUFRBI}"
export DISCORD_CLIENT_SECRET="${DISCORD_CLIENT_SECRET:-test-client-secret}"
export PORT="${PORT:-8080}"
export SITE_URL="${SITE_URL:-http://localhost:${PORT}}"
export INVITE_URL="${INVITE_URL:-https://discord.com/api/oauth2/authorize?client_id=${DISCORD_CLIENT_ID}&permissions=0&scope=applications.commands%20bot}"

# The two the service needs that no older name covers: it signs sessions with
# `SECRET_KEY_BASE` (a fixed value, so a restart does not sign everybody out), and
# it expects a `Secure` cookie by default, which a browser on http will not send
# back to the local site.
export SECRET_KEY_BASE="${SECRET_KEY_BASE:-virtualcrypto3-dev-session-secret}"
export SECURE_COOKIES="${SECURE_COOKIES:-false}"

echo "discord app: $DISCORD_CLIENT_ID ($(jq -r '.name' <<<"$application"))"
echo "public key:  $DISCORD_PUBLIC_KEY"
echo "in guilds:   $(jq -r '.approximate_guild_count // 0' <<<"$application")"
echo "listening:   http://localhost:${PORT}"
