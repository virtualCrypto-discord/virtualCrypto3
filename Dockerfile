# The SPA, which the API serves out of `web/dist`.
FROM node:24-slim AS web

WORKDIR /src/web

# The lockfile is the input. `.npmrc` carries `include=dev`, because npm reads
# `NODE_ENV=production` — which this project's development shell sets — as a reason
# to skip every devDependency, vite included, and reports success either way.
COPY web/package.json web/package-lock.json web/.npmrc ./
RUN npm ci

COPY web/ ./
RUN npm run build

# The server.
FROM rust:1.98.1-slim AS build

WORKDIR /src

# `vc-server` is what the image runs; `register-commands` is what the release command
# runs after the migrations. The queries in it are checked at compile
# time, and `.sqlx` is the committed result of that check, which is why no database is
# needed here — the reason that directory is in the repository at all.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ ./crates/
COPY i18n/ ./i18n/
COPY .sqlx/ ./.sqlx/

ENV SQLX_OFFLINE=true

RUN cargo build --release -p vc-server -p vc-api --bin vc-server --bin register-commands

# For the release command. `--locked` because an install that resolves something else
# is not the one this was tested with.
RUN cargo install sqlx-cli --version 0.9.0 --locked \
      --no-default-features --features rustls,postgres

# The same family as the build stage, and not merely a slim one. `sqlx-cli` links
# against a glibc newer than bookworm's 2.36 — it fails with `GLIBC_2.39 not found`
# when run there — so the runtime has to be the distribution the binaries were built
# on, or the release command fails on every deploy.
FROM debian:trixie-slim

# reqwest verifies TLS against these, and everything this service talks to is https —
# Discord, and the webhook emitter.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# `WEB_ROOT` defaults to the relative `web/dist`, so what decides where the assets
# have to be is the working directory. Nothing has to be set for that to be true.
WORKDIR /app

COPY --from=build /src/target/release/vc-server /app/bin/vc-server
COPY --from=build /src/target/release/register-commands /app/bin/register-commands
COPY --from=build /usr/local/cargo/bin/sqlx /app/bin/sqlx
COPY crates/vc-core/migrations /app/migrations
COPY --from=web /src/web/dist /app/web/dist

# The release command the deployed application already names, provided here so that
# the configuration does not have to change. Arguments are passed to `sqlx migrate run`.
RUN printf '#!/bin/sh\nset -eu\nexec /app/bin/sqlx migrate run --source /app/migrations "$@"\n' \
      > /app/bin/migrate \
    && chmod +x /app/bin/migrate

# The release command both deployments name: the migrations, then the slash commands.
# Discord's PUT is a bulk overwrite, so running it on every deploy keeps the
# application's list equal to the code's. Arguments are passed to the migrations.
RUN printf '#!/bin/sh\nset -eu\n/app/bin/migrate "$@"\nexec /app/bin/register-commands\n' \
      > /app/bin/release \
    && chmod +x /app/bin/release

CMD ["/app/bin/vc-server"]
