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

lint:
	cargo clippy --all-targets --all-features -- -D warnings

build:
	cargo build --workspace

test:
	cargo nextest run --workspace

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
tunnel:
	cloudflared tunnel --url http://localhost:{{port}}
