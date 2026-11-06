#!/usr/bin/env bash
# Compile Grove en release et l'emballe dans dist/grove-<version>-<arch>-linux.tar.gz
# avec son lanceur, son icône et install.sh, pour l'installer sans Rust ni dépôt :
#   tar xf grove-*.tar.gz && ./grove/install.sh
# Le binaire exige au moins la glibc de la machine qui l'a compilé.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --locked -p grove-app --manifest-path "$root/Cargo.toml"

version="$(git -C "$root" describe --always --dirty 2>/dev/null || echo '?')"
name="grove-$version-$(uname -m)-linux"
dist="$root/dist"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

pkg="$stage/grove"
install -Dm 755 "$root/target/release/grove" "$pkg/grove"
install -Dm 755 "$root/scripts/install.sh" "$pkg/install.sh"
install -Dm 644 "$root/packaging/linux/grove.desktop" "$pkg/packaging/linux/grove.desktop"
install -Dm 644 "$root/packaging/linux/grove.svg" "$pkg/packaging/linux/grove.svg"
echo "$version" > "$pkg/VERSION"

install -d "$dist"
tar -C "$stage" -czf "$dist/$name.tar.gz" grove
glibc="$(objdump -T "$pkg/grove" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1 || true)"
echo "Archive : $dist/$name.tar.gz ($(du -h "$dist/$name.tar.gz" | cut -f1), ${glibc:-glibc ?} minimum)"
