# Grove

Terminal multi-projets orienté Claude Code. Voir [specs.md](specs.md).

## État

Phases 0 à 3 : terminal, sidebar, onglets, branches, restauration ; onglets
Claude suivis par les hooks, usage du compte ; worktrees (création, promotion,
suppression, import).

| Crate | Rôle |
|---|---|
| `grove-core` | Modèle (projets, espaces de travail, onglets), config, persistance de l'état |
| `grove-term` | PTY + émulation `alacritty_terminal`, couleurs résolues, sans dépendance à l'UI |
| `grove-claude` | Hooks et statusline de Claude Code (socket Unix), état des sessions, transcripts |
| `grove-git` | Détection du dépôt et lecture de `HEAD` (`gix`), surveillance de `HEAD` (`notify`) |
| `grove-app` | Binaire `grove` (GPUI) |

## Compiler

Dépendances système (Ubuntu/Debian) :

```
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libfontconfig-dev
```

```
cargo run -p grove-app [-- <dossier>]   # restaure la session ; <dossier> est ajouté comme projet
cargo test --workspace
```

## Raccourcis

| Touche | Action |
|---|---|
| `Ctrl+Shift+T` / `Ctrl+Shift+W` | Nouvel onglet / fermer l'onglet |
| `Ctrl+Tab`, `Ctrl+PageDown` / `Ctrl+Shift+Tab`, `Ctrl+PageUp` | Onglet suivant / précédent |
| `Ctrl+Shift+PageUp/PageDown` | Déplacer l'onglet (ou glisser-déposer) |
| `Ctrl+Shift+R` | Renommer l'onglet (ou double-clic ; titre vide = titre du programme) |
| `Ctrl+Shift+O` | Ajouter un projet |
| `Ctrl+Shift+B` | Afficher / masquer la sidebar |
| `Ctrl+Shift+A` | Nouvel onglet Claude |
| `Ctrl+Shift+J` | Aller à la prochaine session Claude qui attend une réponse |
| `Ctrl+Shift+I` | Afficher / masquer le panneau de session |
| `Ctrl+Shift+H` | Sessions passées de l'espace (clic = reprendre) |
| `Ctrl+Shift+G` | Sélecteur de branches (chercher, changer, créer depuis la branche courante ou la branche principale, supprimer, déplacer le travail dans un worktree) |
| `Ctrl+Shift+N` | Nouvel espace de travail (worktree) ; `Maj+Entrée` l'ouvre avec Claude |
| `Ctrl+Shift+V` | Coller |
| `Shift+PageUp/PageDown` | Défiler dans le scrollback |

Clic milieu sur un onglet : le fermer.

Champs de saisie (recherche de branche, nouvel espace, renommage) : `Ctrl+A`
sélectionne tout, `Ctrl+C` / `Ctrl+X` copient / coupent (tout le texte sans
sélection), `Ctrl+V` colle, `Ctrl+Retour arrière` efface le dernier segment.
Un clic en dehors d'un panneau flottant le ferme.

Bouton de synchronisation (à droite de la branche, si le dépôt a un remote) :
comme dans VS Code, il récupère le distant, tire en fast-forward (`↓`) puis
pousse (`↑`) ; une branche sans branche suivie est publiée (`push -u`). Si la
branche locale et la distante ont divergé, rien n'est fait et un message le dit.

## Fichiers

- `~/.config/grove/config.toml` : configuration (`[font] family/size`, `[terminal] shell/scrollback`)
- `~/.local/share/grove/state.json` : état restauré au démarrage (phase 1)

`GROVE_CONFIG_DIR` et `GROVE_DATA_DIR` remplacent ces dossiers.

## Worktrees

Configuration par projet dans `<projet>/.grove.toml` (optionnel) :

```toml
[worktree]
location = "sibling"          # "sibling" : ../<repo>.worktrees/<branche> ; "inside" : .worktrees/<branche>
copy = [".env", ".env.local"] # copiés depuis le checkout principal
setup = ["npm install"]       # lancés dans un onglet « setup » ; {{repo}} = nom du dossier du projet
```

## Claude Code

Un onglet Claude lance `claude --settings ~/.local/share/grove/claude-settings.json`
via le shell interactif (alias et fonctions conservés ; commande réglable par
`[claude] command`). Ce fichier ajoute des hooks et une statusline qui appellent
`grove hook` / `grove statusline` : ils transmettent le JSON reçu au socket de
l'app (`$XDG_RUNTIME_DIR/grove-<pid>.sock`). La statusline de l'utilisateur
(`~/.claude/settings.json`) est appelée en chaîne. `[claude] notifications = false`
coupe les notifications système.

`[claude] usage_api = true` lit aussi l'usage du compte (limites 5 h et semaine)
via l'endpoint interne d'Anthropic, avec le jeton de Claude Code, toutes les 3 min.
C'est hors des points d'intégration officiels, d'où l'option désactivée par défaut
(cf. specs §8.5).

`RUST_LOG=grove=debug` affiche les événements reçus.

## Notes

- `patches/xattr` : `xattr 0.2.3` (tiré par `gpui_http_client`) ne compile plus avec un `libc` récent ; patch d'une ligne.
