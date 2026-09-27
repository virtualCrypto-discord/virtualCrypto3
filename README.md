# virtualCrypto

- [Authorization model and legacy v2 compatibility](docs/authorization.md)
- [Human + Codex review workbench](tools/review-workbench/README.md) — run `just review`
  for review conversations, code changes, verification and human decision records.

## The build cache (mbx)

Rust builds in this checkout go through [mbx](https://mr-boxington.jdx.dev), a
shared Cargo cache. `flake.nix` fetches the binary and puts a `cargo` shim ahead
of the toolchain in the dev shell, so nothing is installed by hand — plain
`cargo build` inside `nix develop` (or direnv) reuses compiled work across
checkouts and worktrees.

What is left to the machine is where the cache lives: a btrfs image, so restoring
an output is a reflink instead of a copy. One command, once per machine:

```sh
sudo bash scripts/mount-mbx-btrfs.sh
```

It creates a sparse image under `~/.local/share/mbx/` (`MBX_IMAGE_SIZE`, 300 GiB
by default), makes the filesystem, loads the module, mounts it at `~/mbx` with
`loop,noatime`, adds the `nofail` entry to `/etc/fstab`, and writes
`~/.config/mbx/config.toml` with `cache_dir` pointing at the mount. Every path is
the invoking user's — the script reads the account behind `sudo`, so `$HOME` is
never assumed — and an existing config file is left alone.

Then check it:

```sh
mbx doctor   # the reflink lines say "cloning is supported", not "different filesystems"
```

### Notes

- Managed target directories default to `<cache_dir>/targets`, on the same
  filesystem — that is what makes the reflink possible. `target/` becomes a
  symlink into that root, kept out of `git status` through `.git/info/exclude`.
  Set `MBX_TARGET_VIEWS=0` to leave `target/` where Cargo puts it.
- The first build fills the store; later builds and other checkouts reuse it.
  Budgets scale with the image: 5% for the store, 10% for managed targets.
- `mkfs.btrfs` comes from nixpkgs (`nix profile add nixpkgs#btrfs-progs`); the
  kernel module is in the WSL kernel but not loaded by default, which the script
  does.
- **`cargo sqlx prepare` cannot be run bare here.** The cache restores every
  artifact, so the compiler never runs — and sqlx learns queries *from the
  compiler*, so it answers "no queries found" and empties `.sqlx`. Touching the
  sources does not help, because the cache does not read mtimes. `just
  sqlx-prepare` is the way: it switches the shim off, compiles the crates that
  carry a query from scratch, and fails rather than leaving an empty directory.
- `mbx cache remove <workspace>` drops one checkout's target and keeps the store;
  `mbx cache stats` reports what is held; `mbx gc --dry-run` previews collection.

## The linker (mold)

`flake.nix` puts [mold](https://github.com/rui314/mold) on the dev shell's PATH
and sets `RUSTFLAGS` to `-C link-arg=-fuse-ld=mold`, so every link inside
`nix develop` — or direnv — goes through it, and CI's links go through it too
because CI is the same shell.

Linking is the part of a rebuild the cache cannot skip: a changed crate in
`vc-core` or `vc-api` is a fresh link of every test binary in the workspace, one
per file. On this checkout's debug builds mold does each one about a quarter
faster than the default `ld` — `vc-server` 1.05 s to 0.78 s, a test binary 1.2 s
to 1.05 s, three runs each — so the saving is that, times however many binaries
the change invalidated.

Nothing is installed by hand and nothing outside the shell is affected: the flag
is GCC's and clang's own, and a build that is not in this flake links with the
default `ld`. The first build after the shell changes its flags is a full rebuild,
because the flags are part of what a compiled crate is keyed on.
