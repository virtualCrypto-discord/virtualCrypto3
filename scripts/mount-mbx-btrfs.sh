#!/usr/bin/env bash
# Give the mbx build cache a filesystem of its own, and keep it mounted across
# restarts. One machine, once:
#
#   sudo bash scripts/mount-mbx-btrfs.sh
#
# Why a filesystem at all: mbx restores compiled outputs by reflinking them out
# of its content-addressed store, and reflinks need copy-on-write file cloning —
# btrfs, XFS, APFS, ReFS. A host whose root filesystem has none (ext4, NTFS)
# still caches, but every restore copies the bytes instead. The store and the
# managed target directories have to be on the same filesystem, which is what
# this image gives them. See the README.
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "run this with sudo: sudo bash $0" >&2
  exit 1
fi

# The invoking user owns the cache, so the paths are theirs. sudo's own `$HOME`
# is root's, which is why this reads the account rather than the environment.
caller="${SUDO_USER:-}"
if [ -z "$caller" ] || [ "$caller" = "root" ]; then
  echo "run this with sudo as the user whose cache it is, not as root directly" >&2
  exit 1
fi

uid=$(id -u "$caller")
gid=$(id -g "$caller")
home=$(getent passwd "$caller" | cut -d: -f6)
image="$home/.local/share/mbx/btrfs.img"
mountpoint="$home/mbx"
size="${MBX_IMAGE_SIZE:-300G}"
fstab=/etc/fstab
entry="$image $mountpoint btrfs loop,noatime,nofail 0 0"

# Sparse: it occupies what is written to it, not what it is sized at.
if [ ! -f "$image" ]; then
  install -d -o "$uid" -g "$gid" -m 755 "$(dirname "$image")"
  sudo -u "$caller" truncate -s "$size" "$image"
  mkfs.btrfs -f -L mbx "$image"
  echo "created $image ($size, sparse)"
fi

if [ ! -d "$mountpoint" ]; then
  install -d -o "$uid" -g "$gid" -m 755 "$mountpoint"
fi

# The filesystem's root inode is not necessarily owned by the person who writes
# the cache into it, so the mount point's owner is carried across. Ownership
# lives on the filesystem, so this is once, not every boot.
if mountpoint -q "$mountpoint"; then
  echo "$mountpoint is already mounted"
else
  modprobe btrfs
  owner=$(stat -c '%u:%g' "$mountpoint")
  mount -o loop,noatime "$image" "$mountpoint"
  chown "$owner" "$mountpoint"
  echo "mounted $image on $mountpoint (owner $owner)"
fi

# An image is a file on the root filesystem, so the mount is redone at boot;
# `nofail` keeps a missing image from holding up the boot.
if grep -qF "$entry" "$fstab"; then
  echo "$fstab already has the entry"
else
  cp -n "$fstab" "$fstab.before-mbx"
  printf '\n# The mbx build cache: a btrfs image, so restores can be reflinked.\n%s\n' "$entry" >> "$fstab"
  echo "added the entry to $fstab"
fi

# The store has to be told where the image is — TOML has no `$HOME`, and a
# procedure that names somebody's home directory is not one. An existing file is
# left alone: it may hold settings this script knows nothing about.
config="$home/.config/mbx/config.toml"
if [ -f "$config" ]; then
  echo "$config exists; leaving it alone"
else
  install -d -o "$uid" -g "$gid" -m 755 "$(dirname "$config")"
  printf '# Written by scripts/mount-mbx-btrfs.sh.\n#\n# The store lives on the btrfs image mounted at %s, and the managed target\n# directories default to <cache_dir>/targets — the same filesystem, which is\n# what lets a restore be a reflink instead of a copy.\ncache_dir = "%s"\n' \
    "$mountpoint" "$mountpoint/cache" > "$config"
  chown "$uid:$gid" "$config"
  echo "wrote $config"
fi

df -hT "$mountpoint"
