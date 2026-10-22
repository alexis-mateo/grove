#!/usr/bin/env bash
# Compile Grove en release et l'installe pour l'utilisateur courant : binaire
# dans ~/.local/bin, icône et lanceur dans ~/.local/share. Relancer le script
# met l'installation à jour ; `--uninstall` la retire.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$bin_dir/grove"
desktop="$data_dir/applications/grove.desktop"
icon="$data_dir/icons/hicolor/scalable/apps/grove.svg"

refresh_caches() {
    update-desktop-database -q "$data_dir/applications" 2>/dev/null || true
    gtk-update-icon-cache -q -t -f "$data_dir/icons/hicolor" 2>/dev/null || true
}

if [[ ${1:-} == --uninstall ]]; then
    rm -f "$bin" "$desktop" "$icon"
    refresh_caches
    echo "Grove désinstallé (configuration et état conservés dans ~/.config/grove et ~/.local/share/grove)."
    exit 0
fi

# Lancé hors d'un shell interactif, cargo n'est pas forcément dans le PATH.
PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --locked -p grove-app --manifest-path "$root/Cargo.toml"

# Copie puis renommage : écrire par-dessus le binaire d'une instance ouverte
# échouerait (« Text file busy ») ; elle garde l'ancien jusqu'à sa fermeture.
install -d "$bin_dir"
install -m 755 "$root/target/release/grove" "$bin.new"
mv -f "$bin.new" "$bin"

install -Dm 644 "$root/packaging/linux/grove.svg" "$icon"
install -d "$(dirname "$desktop")"
sed "s|@BIN@|$bin|" "$root/packaging/linux/grove.desktop" > "$desktop"
refresh_caches

echo "Grove installé : $bin ($(git -C "$root" describe --always --dirty 2>/dev/null || echo '?'))"
if pgrep -x grove >/dev/null; then
    echo "Une instance tourne encore avec l'ancienne version : la relancer pour passer à la nouvelle."
fi
