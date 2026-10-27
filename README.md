# Grove

Terminal multi-projets orienté Claude Code. Voir [specs.md](specs.md).

## État

Phases 0 à 3 : terminal, sidebar, onglets, branches, restauration ; onglets
Claude suivis par les hooks, usage du compte ; worktrees (création, promotion,
suppression, import). Phase 4 commencée : vue native des sessions Claude
(stream-json, permissions, bascule avec le terminal). Phase 5 : profils de
terminal (shell, SSH, commande), page Paramètres et liste des raccourcis.

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

## Installer

```
./scripts/install.sh               # compile en release et installe (ou met à jour)
./scripts/install.sh --uninstall
```

Le binaire va dans `~/.local/bin/grove`, l'icône et le lanceur (`grove.desktop`,
associé à la fenêtre par son `app_id`) dans `~/.local/share`. Pour mettre à jour :
`git pull` puis relancer le script, puis redémarrer Grove ; une instance ouverte
garde l'ancien binaire jusqu'à sa fermeture.

## Raccourcis

| Touche | Action |
|---|---|
| `Ctrl+Shift+T` / `Ctrl+Shift+W` | Nouvel onglet (profil par défaut) / fermer l'onglet |
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
| `Ctrl+Shift+M` | Onglet Claude : basculer entre le terminal (TUI officiel) et la vue native |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copier la sélection / coller |
| `Shift+PageUp/PageDown` | Défiler dans le scrollback |
| `Ctrl+,` | Paramètres (`Échap` les ferme) |

La liste complète, avec recherche, est dans Paramètres › Raccourcis clavier
(icône clavier en pied de sidebar) : elle est générée depuis les raccourcis
enregistrés.

Clic milieu sur un onglet : le fermer.

Sélection dans le terminal : glisser, double-clic (mot), triple-clic (ligne).
Elle devient la sélection primaire, collée par un clic milieu. Quand le
programme capte la souris (vim, htop, certains TUI), `Shift` garde la
sélection pour Grove.

Liens du terminal (URL du texte et hyperliens OSC 8, schémas `http(s)`,
`file`, `mailto`, `ftp`, `ssh`, `git`) : soulignés au survol avec `Ctrl`,
ouverts par `Ctrl+clic` ; l'adresse s'affiche en bas à gauche.

Lettres accentuées et touches mortes (`^` puis `e`…) : le texte composé par la
méthode de saisie (IBus, XIM, compose) arrive dans le terminal, la vue native
et les champs de saisie ; la composition en cours s'affiche soulignée au curseur.

Images : protocole graphique de kitty, en placements virtuels (caractères
U+10EEEE, le mode de Claude Code et de `kitten icat --unicode-placeholder`).
Images PNG ou pixels bruts, transmises en ligne, par fichier ou par mémoire
partagée. Les placements classiques (image posée au curseur) ne sont pas
affichés. Claude Code n'active les images que pour kitty et Ghostty : Grove
lance ses PTY avec `CLAUDE_CODE_FORCE_TERMINAL_IMAGES=1`, ce qui affiche par
exemple les vignettes du plugin `claude-image-view`.

Champs de saisie (recherche, nouvel espace, renommage, Paramètres) : flèches,
`Début` / `Fin`, `Ctrl+←/→` de mot en mot, `Shift` pour sélectionner ; `Ctrl+A`
sélectionne tout, `Ctrl+C` / `Ctrl+X` copient / coupent (tout le texte sans
sélection), `Ctrl+V` colle, `Ctrl+Retour arrière` / `Ctrl+Suppr` effacent un mot.
Un clic en dehors d'un panneau flottant le ferme.

Bouton de synchronisation (à droite de la branche, si le dépôt a un remote) :
comme dans VS Code, il récupère le distant, tire en fast-forward (`↓`) puis
pousse (`↑`) ; une branche sans branche suivie est publiée (`push -u`). Si la
branche locale et la distante ont divergé, rien n'est fait et un message le dit.

## Fichiers

- `~/.config/grove/config.toml` : configuration (`[font]`, `[terminal]`, `[[terminal.profiles]]`, `[claude]`),
  modifiable depuis Paramètres (`Ctrl+,` ou l'engrenage en pied de sidebar). La page
  écrit le fichier en gardant commentaires et mise en forme ; une modification à la
  main s'applique sans redémarrer (police des terminaux ouverts comprise ; shell,
  scrollback et commandes valent pour les prochains onglets).
- `~/.local/share/grove/state.json` : état restauré au démarrage (phase 1)

`GROVE_CONFIG_DIR` et `GROVE_DATA_DIR` remplacent ces dossiers.

## Profils de terminal

Un profil décrit ce qu'un onglet lance : le shell, une connexion SSH ou une
commande. Flèche à côté du `+` (ou clic droit sur le `+`) : liste des profils,
avec recherche. `Ctrl+Shift+T` ouvre le profil par défaut.

```toml
[terminal]
default_profile = "staging"      # optionnel ; sinon le shell

[[terminal.profiles]]
name = "staging"
kind = "ssh"                     # déduit sinon : host → ssh, command → commande
host = "staging-web-1"           # alias de ~/.ssh/config ou nom d'hôte
user = "deploy"                  # optionnels : port, identity, jump, options = ["ServerAliveInterval=30"]
remote_command = "cd /var/www/app && exec $SHELL -l"
reconnect = "auto"               # au redémarrage ; par défaut, bouton « Se connecter »

[[terminal.profiles]]
name = "db"
command = "docker compose exec db psql -U app"
cwd = "backend"                  # relatif à l'espace de travail
env = { PGUSER = "app" }
```

- Profils globaux dans `config.toml`, par projet dans `.grove.toml` (même
  syntaxe, versionnable) ; un profil de projet masque un profil global du même nom.
- Les entrées `Host` de `~/.ssh/config` (hors motifs) sont proposées telles
  quelles ; un profil Grove qui vise le même hôte les remplace.
- SSH passe par la commande `ssh` du système : `~/.ssh/config`, agent, clés,
  `known_hosts` et `ProxyJump` fonctionnent comme d'habitude. Grove ne stocke
  aucun mot de passe : `ssh` les demande dans le terminal.
- Onglet SSH : icône serveur, titre = nom du profil. Connexion fermée : bandeau
  « Connexion fermée (code n) » avec Reconnecter. Au redémarrage, les onglets
  SSH sont restaurés sans se connecter (bouton ou `Entrée`).

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

### Vue native

`Ctrl+Shift+M` (ou le bouton de la barre d'onglets) bascule l'onglet Claude
actif : le processus courant s'arrête et l'autre vue reprend la session
(`--resume`). `[claude] view = "native"` ouvre les nouveaux onglets Claude dans
cette vue. Elle pilote `claude -p --input-format stream-json --output-format
stream-json`, lancé par le shell interactif comme en mode terminal :

- fil de la conversation (markdown simple, appels d'outils repliables, différences
  pour `Edit`), relu dans le transcript pour une session reprise ;
- `Entrée` envoie, `Maj+Entrée` ajoute une ligne, `↑` / `↓` rappellent les prompts,
  `Échap` interrompt (le processus s'arrête, le message suivant reprend la session) ;
- `Maj+Tab` change le mode de permission (demander, modifications auto, plan), appliqué
  par relance du processus ;
- les demandes de permission passent par `--permission-prompt-tool` : Claude Code lance
  `grove mcp-permission` (serveur MCP sur stdio), qui transmet la demande au socket et
  attend la réponse (Autoriser, Toujours pour cet outil pendant la session, Refuser).
  Si Grove ne répond pas, la demande est refusée.

`RUST_LOG=grove=debug` affiche les événements reçus.

## Notes

- `patches/xattr` : `xattr 0.2.3` (tiré par `gpui_http_client`) ne compile plus avec un `libc` récent ; patch d'une ligne.
