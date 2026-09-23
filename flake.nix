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

        # mbx — the shared Cargo build cache — is not in nixpkgs, and it ships a
        # static binary for each platform. Building it from source would mean a
        # Cargo build in front of the cache that exists to skip Cargo builds, so
        # the release is fetched instead. The hashes are the release's own
        # SHA256SUMS entries and the archives are musl-static, so there is
        # nothing to patch. Linux only: the archive names are per platform, and
        # these are the ones this dev shell runs.
        mbxReleases = {
          x86_64-linux = {
            arch = "x86_64";
            hash = "sha256-4EjlrllX/hhQbh2v0Unogf43PJ4cDTrXqUpUFJQgzLs=";
          };
          aarch64-linux = {
            arch = "aarch64";
            hash = "sha256-vbFa4MxqEk53ClPsUqwI9toMxXTv5Nx/eHiE6tjaNfU=";
          };
        };

        mbx = pkgs.stdenvNoCC.mkDerivation (finalAttrs: {
          pname = "mbx";
          version = "1.14.0";

          src = pkgs.fetchurl {
            url = "https://github.com/jdx/mr-boxington/releases/download/v${finalAttrs.version}/mbx-${mbxReleases.${system}.arch}-unknown-linux-musl.tar.gz";
            hash = mbxReleases.${system}.hash;
          };

          sourceRoot = ".";
          dontConfigure = true;
          dontBuild = true;

          installPhase = ''
            runHook preInstall
            install -Dm755 mbx $out/bin/mbx
            runHook postInstall
          '';

          meta = {
            description = "A shared cache for Cargo builds";
            homepage = "https://mr-boxington.jdx.dev";
            license = pkgs.lib.licenses.mit;
            mainProgram = "mbx";
            platforms = [
              "x86_64-linux"
              "aarch64-linux"
            ];
            sourceProvenance = [ pkgs.lib.sourceTypes.binaryNativeCode ];
          };
        });

        mbxTools = pkgs.lib.optionals (builtins.hasAttr system mbxReleases) [ mbx ];

        # The Cargo shim is a name, not a program: `mbx` decides what it is from
        # argv[0], so a symlink called `cargo` is the whole of it. Ahead of the
        # toolchain on the dev shell's PATH, plain `cargo build` goes through the
        # cache — no `mbx setup`, no `~/.cargo/config.toml`, nothing global that
        # outlives the shell it applies to.
        mbxCargo = pkgs.runCommand "mbx-cargo-shim" { } ''
          mkdir -p $out/bin
          ln -s ${mbx}/bin/mbx $out/bin/cargo
        '';

        # First, so its `cargo` is found before the toolchain's.
        mbxShims = pkgs.lib.optionals (builtins.hasAttr system mbxReleases) [ mbxCargo ];

        rustTools = with pkgs; [
          rustToolchain
          cargo-watch
          cargo-nextest
          cargo-deny
          cargo-insta
          sqlx-cli
        ];

        serviceTools = with pkgs; [
          flyctl
          just
          direnv
          cloudflared
          curl
          jq
        ];

        deps = with pkgs; [
          openssl
          postgresql_17
          pkg-config
          # The linker the shell builds with, named by `env` below. GCC has taken
          # `-fuse-ld=mold` since 12, so this needs no clang beside it.
          mold
        ];
      in
      {
        formatter = pkgs.nixpkgs-fmt;

        # `mbx` alone, for `nix profile install .#mbx` and `nix run .#mbx`.
        packages = pkgs.lib.optionalAttrs (builtins.hasAttr system mbxReleases) {
          inherit mbx;
        };

        devShells.default = pkgs.mkShell {
          packages = mbxShims ++ rustTools ++ serviceTools ++ deps ++ mbxTools;

          env = {
            DATABASE_URL = "postgres://postgres:postgres@localhost:5432/virtualcrypto_dev";

            # Linking, which is the half of a rebuild the shared cache cannot help with: a
            # changed crate is a fresh link every time, and this workspace links one test
            # binary per file. Measured on this checkout's debug builds, one link is about a
            # quarter faster than the default `ld` — `vc-server` 1.05 s to 0.78 s, a test
            # binary 1.2 s to 1.05 s — so the saving is what those add up to after a crate
            # everything depends on moves.
            #
            # `-fuse-ld` is GCC's and clang's own switch, so it goes through whichever driver
            # the toolchain uses. It is set here rather than in `.cargo/config.toml` because
            # this shell is what a laptop and CI have in common: a build that is not in it
            # links the default way.
            RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
          };

          shellHook = ''
            echo "virtualCrypto dev shell — $(rustc --version)"
            echo "postgres: $(postgres --version)"
            echo "linker: $(mold --version | head -n 1)"
          '';
        };
      }
    );
}
