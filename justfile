set shell := ["bash", "-euo", "pipefail", "-c"]

export DATABASE_URL := "postgres://postgres:postgres@localhost:5432/virtualcrypto_dev"

db_name := "virtualcrypto_dev"
port := "8080"

# Create the databases used by the service and the schema-parity check.
db-create:
	createdb -h localhost -U postgres {{db_name}} || true
	createdb -h localhost -U postgres virtualcrypto_baseline || true
	createdb -h localhost -U postgres virtualcrypto_test || true

# Apply sqlx migrations.
migrate:
	sqlx migrate run --source crates/vc-core/migrations

fmt:
	cargo fmt --all

# The gates CI runs, in the order it runs them.
check: fmt-check lint test sqlx-check baseline-check

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

build:
	cargo build --workspace

test:
	cargo nextest run --workspace

# The committed offline query data has to match the queries and the schema. A
# deployment builds with SQLX_OFFLINE, so data that has drifted is a build that
# succeeds here and fails there — or worse, one that succeeds with the wrong SQL.
sqlx-check:
	cargo sqlx prepare --workspace --check

# Verify the sqlx baseline reproduces the Ecto schema and is idempotent.
baseline-check:
	bash scripts/baseline-check.sh

# Show the Discord application `.env` points at, without starting anything.
discord-info:
	. scripts/discord-env.sh

# Run the service against the Discord test application described by `.env`.
dev:
	. scripts/discord-env.sh && cargo run -p vc-server

# Expose the local server on a public https URL so Discord can reach it.
# The quick form: a different random hostname on every run.
tunnel:
	cloudflared tunnel --url http://localhost:{{port}}

# Authorise cloudflared against your Cloudflare account (one time, opens a browser).
tunnel-login:
	cloudflared tunnel login

# Run the named tunnel `vcrypto3-dev`, which serves
# https://vcrypto-dev.tignear.com from the ingress in ~/.cloudflared/config.yml.
# The Discord Interactions Endpoint URL is that host plus
# /api/integrations/discord/interactions.
# Run it in the foreground for QA; `tunnel-service` keeps it up in the background.
tunnel-named:
	cloudflared tunnel run vcrypto3-dev

# Install, enable and start the user-level systemd unit that keeps the tunnel up.
# It runs in your own systemd session, so no root is involved anywhere.
tunnel-service:
	mkdir -p ~/.config/systemd/user
	install -m 644 systemd/cloudflared.service ~/.config/systemd/user/cloudflared.service
	systemctl --user daemon-reload
	systemctl --user enable --now cloudflared
	systemctl --user --no-pager status cloudflared

# Follow the tunnel service log.
tunnel-logs:
	journalctl --user -u cloudflared -f
