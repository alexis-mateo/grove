# Grove

Terminal multi-projets orienté Claude Code. Voir [specs.md](specs.md).

## État

Phases 0 à 3 : terminal, sidebar, onglets, branches, restauration ; onglets
Claude suivis par les hooks, usage du compte ; worktrees (création, promotion,
suppression, import). Phase 4 commencée : vue native des sessions Claude
(stream-json, permissions, bascule avec le terminal). Phase 5 : profils de
terminal (shell, SSH, commande), page Paramètres et liste des raccourcis.
Phase 6 commencée : pull requests ouvertes (à reviewer, les miennes), reviews et
commentaires confiés à Claude. Aussi : recherche dans le scrollback, palette de
commandes, vue diff des modifications, usage Claude par jour et par projet.

| Crate | Rôle |
|---|---|
| `grove-core` | Modèle (projets, espaces de travail, onglets), config, persistance de l'état |
| `grove-term` | PTY + émulation `alacritty_terminal`, couleurs résolues, sans dépendance à l'UI |
| `grove-claude` | Hooks et statusline de Claude Code (socket Unix), état des sessions, transcripts |
| `grove-git` | Détection du dépôt et lecture de `HEAD` (`gix`), surveillance de `HEAD` (`notify`), branches, worktrees et diff (CLI `git`) |
| `grove-github` | Jeton (saisi ou `gh auth token`), pull requests ouvertes via l'API GraphQL |
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

## Distribuer

```
./scripts/package.sh               # → dist/grove-<version>-x86_64-linux.tar.gz
```

L'archive contient le binaire, le lanceur, l'icône et `install.sh`, qui
l'installe sans compiler (ni Rust ni dépôt requis) :

```
tar xf grove-*.tar.gz && ./grove/install.sh    # même commande pour mettre à jour
./grove/install.sh --uninstall
```

Le binaire exige au moins la glibc de la machine qui l'a compilé (affichée par
le script) et un pilote Vulkan (`mesa-vulkan-drivers`).

## Raccourcis

| Touche | Action |
|---|---|
| `Ctrl+Shift+T` / `Ctrl+Shift+W` | Nouvel onglet (profil par défaut) / fermer le volet actif (l'onglet s'il n'est pas partagé) |
| `Ctrl+Shift+D` / `Ctrl+Shift+S` | Partager le volet : nouveau terminal à droite / en dessous |
| `Alt+←/→/↑/↓` | Passer au volet voisin (sans split, la touche va au programme) |
| `Ctrl+Tab`, `Ctrl+PageDown` / `Ctrl+Shift+Tab`, `Ctrl+PageUp` | Onglet suivant / précédent |
| `Ctrl+Shift+PageUp/PageDown` | Déplacer l'onglet (ou glisser-déposer) |
| `Ctrl+Shift+R` | Renommer l'onglet (ou double-clic ; titre vide = titre du programme) |
| `Ctrl+Shift+O` | Ajouter un projet |
| `Ctrl+Shift+B` | Afficher / masquer la sidebar |
| `Ctrl+Shift+A` | Nouvel onglet Claude |
| `Ctrl+Shift+J` | Aller à la prochaine session Claude qui attend une réponse |
| `Ctrl+Shift+I` | Afficher / masquer le panneau de session (modèle, coût, contexte, tâches planifiées) |
| `Ctrl+Shift+H` | Sessions passées de l'espace (clic = reprendre) |
| `Ctrl+Shift+G` | Sélecteur de branches (chercher, changer, créer depuis la branche courante ou la branche principale, supprimer, déplacer le travail dans un worktree) |
| `Ctrl+Shift+N` | Nouvel espace de travail (worktree) ; `Maj+Entrée` l'ouvre avec Claude |
| `Ctrl+Shift+M` | Onglet Claude : basculer entre le terminal (TUI officiel) et la vue native |
| `Ctrl+Shift+P` | Pull requests GitHub (`Échap` ferme) |
| `Ctrl+Shift+K` | Palette de commandes : toutes les actions et les espaces de travail, filtrés au fil de la frappe |
| `Ctrl+Shift+E` | Modifications de l'espace : fichiers modifiés et leur diff (ou clic sur `●3` à côté de la branche) |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copier la sélection / coller |
| `Shift+PageUp/PageDown` | Défiler dans le scrollback |
| `Ctrl+Shift+F` | Chercher dans le scrollback du terminal |
| `Ctrl+,` | Paramètres (`Échap` les ferme) |

La liste complète, avec recherche, est dans Paramètres › Raccourcis clavier
(icône clavier en pied de sidebar) : elle est générée depuis les raccourcis
enregistrés.

Clic milieu sur un onglet : le fermer.

Boutons précédent / suivant de la souris : onglet visité avant ou après, comme
dans un navigateur, d'un espace de travail à l'autre. Les onglets fermés depuis
sont sautés ; aller sur un nouvel onglet oublie les « suivants ».

Volets (splits) : un onglet peut être partagé en plusieurs terminaux, côte à
côte ou empilés, à volonté. Le nouveau volet lance le même profil que celui
qu'on partage (le profil par défaut à côté d'une session Claude). Le volet
actif a un liseré ; un clic en change. Les séparateurs se déplacent à la
souris (double-clic : moitié-moitié). Disposition et proportions sont
restaurées au redémarrage ; un shell qui quitte ferme son volet.

Sélection dans le terminal : glisser, double-clic (mot), triple-clic (ligne).
Elle devient la sélection primaire, collée par un clic milieu. Quand le
programme capte la souris (vim, htop, certains TUI), `Shift` garde la
sélection pour Grove.

Recherche dans le scrollback (`Ctrl+Shift+F`, préremplie avec la sélection
si elle tient sur une ligne) : texte littéral, casse ignorée tant qu'il n'a pas
de majuscule. Elle part du bas de l'écran ; `Entrée` ou `↑` remonte vers les
plus anciennes, `Maj+Entrée` ou `↓` redescend, en boucle. La correspondance en
cours est sélectionnée (`Ctrl+Shift+C` la copie), les autres visibles sont
surlignées ; `Échap` ferme.

Vue diff (`Ctrl+Shift+E`) : les fichiers modifiés de l'espace par rapport à
`HEAD` (indexés, non indexés, non suivis, conflits), avec les lignes ajoutées et
supprimées, et le diff du fichier choisi (`↑`/`↓` pour changer de fichier). Elle
se relit quand l'état git change (au plus toutes les 15 s) ou avec le bouton.

Liens du terminal (URL du texte et hyperliens OSC 8, schémas `http(s)`,
`file`, `mailto`, `ftp`, `ssh`, `git`) : soulignés au survol avec `Ctrl`,
ouverts par `Ctrl+clic` ; l'adresse s'affiche en bas à gauche. Les chemins de
fichiers ou de dossiers existants aussi (absolus, sous `~/`, ou relatifs au
dossier de l'espace de travail, avec ou sans `:ligne` ou `:ligne:colonne`) :
les dossiers avec l'application du système, les fichiers aussi, sauf si une
commande est réglée (Paramètres › Terminal, ou dans `config.toml`) :

```toml
[terminal]
open_files_with = "code -g {path}:{line}:{column}"   # lancé par sh -c ; sans ligne, « :{line} » est retiré
```

Si la commande échoue (éditeur absent…), Grove se rabat sur l'application du système.

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
- `~/.local/share/grove/github-token` : jeton GitHub saisi dans les Paramètres (droits 600)

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

Les worktrees créés ou supprimés hors de Grove (Claude Code, `git worktree` dans
un terminal) apparaissent et disparaissent aussitôt de la sidebar : `.git/worktrees`
est surveillé. Un espace où une session Claude travaille est gardé jusqu'au
changement suivant.

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

### Usage par jour et par projet

Palette (« Usage Claude ») ou lien sous la carte d'usage de la sidebar : coût et
tokens par jour, par projet et par modèle, sur 7, 30 ou 90 jours, pour le projet
actif ou tous. Lu dans les transcripts (`~/.claude/projects`, sous-agents
compris) : chaque réponse de l'API y donne ses tokens, rangée par jour local et
par projet de Grove selon son dossier (racine, worktrees) ; les autres dossiers
apparaissent sous leur chemin. Relecture incrémentale : seules les lignes
ajoutées sont relues.

Le coût est **estimé** au tarif public de l'API (entrée, sortie, écriture du
cache à 1,25× ou 2× l'entrée selon sa durée, lecture du cache) : avec un
abonnement claude.ai, ce n'est pas ce qui est facturé. Comparé au coût de session
que Claude Code enregistre (`cost-state`), l'écart constaté est de quelques %.
Un modèle sans tarif connu est compté en tokens seulement, et signalé.

## Pull requests GitHub

`Ctrl+Shift+P` (ou l'icône PR en pied de sidebar, avec le nombre de reviews
demandées) : les PR ouvertes qui me concernent, du projet actif ou de tous.

- **À reviewer** : review demandée (à moi ou à une de mes équipes) ou déjà
  donnée ; badge « à reviewer », « nouveaux commits » ou « nouvelles réponses »
  (depuis ma review), « changements demandés, en attente de l'auteur »,
  « review envoyée ».
- **Mes pull requests** : badge « n commentaires à traiter » (fils non résolus
  où le reviewer a parlé en dernier, sans commit poussé depuis), « changements
  demandés » (sans commit poussé depuis), « traitée, en attente du reviewer »
  (j'ai répondu ou poussé), « approuvée », « brouillon ». Le bouton n'apparaît
  que s'il reste quelque chose à traiter.
- Pastille de CI (réussie, en échec, en cours) ; clic sur une ligne : la PR
  dans le navigateur. Dans la sidebar, l'espace de travail sur la branche
  d'une PR affiche son numéro et sa CI.
- **Reviewer** / **Actualiser la review** et **Prendre en compte les
  commentaires** ouvrent un onglet Claude dans l'espace de la branche, ou dans
  un worktree créé pour l'occasion (branche récupérée sur le remote ;
  `refs/pull/<n>/head` dans `pr-<n>` pour un fork). Le skill ou le prompt
  reçoit l'URL de la PR, et les fils qui attendent ma réponse (fichier, ligne,
  auteur, texte) pour les commentaires.
- Lecture toutes les 2 min (une requête GraphQL), sans notification.

Jeton : celui de la CLI GitHub (`gh auth token`, cherché aussi via le shell
interactif), ou un jeton saisi dans Paramètres › GitHub, qui le remplace.
Le projet d'une PR est celui dont un remote pointe sur son dépôt.

Les deux actions envoient un skill ou un prompt libre (Paramètres › GitHub) :
un texte qui commence par `/` est un skill, lancé avec l'URL de la PR en
argument ; sinon, l'URL suit le prompt. Les fils qui attendent ma réponse sont
ajoutés pour les commentaires.

```toml
[github.review]
prompt = "/review"              # un skill, ou un prompt libre
refresh_prompt = "L'auteur a répondu ou poussé de nouveaux commits, mets la review à jour."

[github.address]
prompt = "Analyse les commentaires de review de cette PR et applique les corrections pertinentes. Propose ensuite dans le chat une réponse concise en anglais pour chaque fil. Ne poste rien sur GitHub avant mon accord : quand je dis go, poste les réponses."
```

`.grove.toml` peut les changer pour un projet (`[github.review] prompt`,
`[github.address] prompt`).

## Notes

- `patches/xattr` : `xattr 0.2.3` (tiré par `gpui_http_client`) ne compile plus avec un `libc` récent ; patch d'une ligne.
