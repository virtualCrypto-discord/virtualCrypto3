{
  description = "virtualCrypto — Discord crypto bot, Rust rewrite (Fly.io + PostgreSQL)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Single source of truth for the toolchain: rust-toolchain.toml.
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        rustTools = with pkgs; [
          rustToolchain
          cargo-watch
          cargo-nextest
          cargo-deny
          sqlx-cli
        ];

        serviceTools = with pkgs; [
          flyctl
          just
          direnv
        ];

        deps = with pkgs; [
          openssl
          postgresql_16
          pkg-config
        ];
      in
      {
        formatter = pkgs.nixpkgs-fmt;

        devShells.default = pkgs.mkShell {
          packages = rustTools ++ serviceTools ++ deps;

          env = {
            DATABASE_URL = "postgres://postgres:postgres@localhost:5432/virtualcrypto_dev";
            PGDATA = ".pgdata";
            PGHOST = "localhost";
            PGPORT = "5432";
          };

          shellHook = ''
            echo "virtualCrypto dev shell — $(rustc --version)"
            echo "postgres: $(postgres --version) (PGDATA=$PGDATA)"
          '';
        };
      }
    );
}
