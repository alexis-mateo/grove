# Grove — Spécifications

> Terminal multi-projets orienté Claude Code.
> Nom retenu : **Grove** (binaire `grove`), provisoire mais adopté pour l'instant (cf. §1.2).
> Statut : brouillon v0.1 — 2026-10-21

---

## 1. Vision

Un client lourd **natif, écrit en Rust**, qui sert de « poste de pilotage » pour travailler sur plusieurs projets en parallèle avec Claude Code :

- une **sidebar** listant les projets (dossiers) et leurs espaces de travail ;
- des **onglets** par espace de travail : terminaux classiques et sessions Claude Code ;
- une **visibilité en temps réel** sur chaque session Claude (en cours / attend une réponse / terminée, contexte, coût, modèle) ;
- la gestion des **branches et des git worktrees**, **en option** : on doit pouvoir travailler directement dans le dossier du projet *ou* dans des worktrees isolés ;
- à terme, l'intégration **GitHub** (pull requests, CI, GitHub Projects).

Inspiration : [cmux](https://github.com/manaflow-ai/cmux) (macOS uniquement, Swift + libghostty). Ici l'objectif est **Linux d'abord**, puis macOS, avec un rendu GPU natif.

### 1.1 Positionnement

| Outil | Limite par rapport au besoin |
|---|---|
| cmux, Conductor | macOS uniquement |
| Crystal | Electron (lourd) |
| Claude Squad | TUI dans tmux, pas de vraie UI |
| tmux / onglets de terminal | aucune conscience de l'état des sessions Claude |

Valeur ajoutée : **natif, rapide, Linux, pensé autour du modèle « projet → espace de travail → sessions »**.

### 1.2 Le nom : Grove

Un *grove* est un **bosquet** : un petit groupe d'arbres qui poussent ensemble au même endroit.

Le nom décrit le modèle de l'outil :

| Dans le bosquet | Dans Grove |
|---|---|
| Le bosquet | L'ensemble de tes projets, réunis dans une seule fenêtre |
| Un arbre | Un projet (repo) |
| Le tronc | L'espace de travail **direct**, le checkout principal |
| Les branches | Les **branches git** et les **worktrees**, qui poussent à partir du tronc sans le gêner |
| Ce qui pousse, travaille dans l'arbre | Les sessions Claude et les terminaux, actifs en parallèle |
| La récolte, l'élagage | Les PR mergées, les worktrees nettoyés |

L'idée sous-jacente : **plusieurs choses qui poussent en même temps, à un seul endroit, et qu'on entretient d'un coup d'œil**.

Pourquoi ce nom :

- **court** (5 lettres), agréable à taper comme commande : `grove`, `grove open .` ;
- **lisible à l'international**, sans connotation technique trop marquée ;
- **sans référence à « Claude »** (marque d'Anthropic), pour pouvoir un jour piloter d'autres agents ;
- il laisse un **univers visuel** simple à exploiter (cf. ci-dessous).

#### Pistes d'identité visuelle et de vocabulaire

- **Icône** : quelques arbres stylisés, ou un arbre dont les branches forment un graphe git.
- **Palette** : graphite neutre (fonds sombres à peine bleutés), un accent émeraude pour l'état actif, ambre pour les sessions qui attendent une réponse, rouge pour les limites et actions destructives (`grove-app/src/theme.rs`).
- **Vocabulaire possible dans l'UI** (à utiliser avec parcimonie pour rester clair) :
  - « Planter » un worktree : créer un espace de travail ;
  - « Élaguer » : nettoyer les worktrees dont la PR est mergée ;
  - indicateurs d'état façon saisons ou feuillage (●, ◐, ○).
- La terminologie technique (projet, espace de travail, worktree, session) reste la référence dans la doc et le code. Le vocabulaire « forêt » n'est qu'un clin d'œil.

#### À vérifier avant de figer le nom

- disponibilité du nom de crate sur **crates.io** (`grove`, sinon `grove-term` ou `grove-app`) ;
- organisation ou repo **GitHub** ;
- **nom de domaine** ;
- collisions avec des outils existants du même nom (il en existe sûrement dans d'autres domaines) ;
- noms de repli : **Orchard**, **Établi**, **Canopy**.

---

## 2. Principes directeurs

1. **Ne pas réimplémenter l'agent.** On s'appuie toujours sur la CLI officielle `claude`, jamais sur l'API Anthropic en direct : on garde les outils, `CLAUDE.md`, les MCP, les hooks, les skills, l'authentification existante.
2. **S'intégrer par les points officiels d'abord** (hooks, statusline, flags CLI). Les formats internes (transcripts JSONL) sont lus de façon défensive, car ils peuvent changer.
3. **Le worktree est une option, pas un prérequis.** Tout s'attache à un *espace de travail* (un dossier). Un worktree n'est qu'un type d'espace de travail.
4. **La session Claude est l'entité, la vue est interchangeable.** Une même session doit pouvoir s'afficher dans un terminal (TUI officiel) ou dans l'interface native.
5. **Découpage strict.** Chaque phase livre un outil utilisable au quotidien.

---

## 3. Choix technique : pourquoi pas Tauri

Tauri a un backend Rust natif, mais **l'interface est une page web** rendue par le WebView du système (WebKitGTK sous Linux, WKWebView sous macOS, WebView2 sous Windows). Pour ce projet, cela implique :

- le terminal serait rendu par **xterm.js** (JavaScript) et non par du code compilé ;
- chaque octet sortant du PTY traverserait un **pont IPC** Rust → WebView (sérialisation, latence), ce qui coûte cher avec Claude Code qui redessine beaucoup l'écran ;
- sous Linux, **WebKitGTK** est le maillon faible : performances WebGL inégales, bugs de rendu, comportement différent selon la distribution ;
- les raccourcis clavier et l'IME passent par la couche navigateur.

Tauri reste viable (le terminal de VS Code repose sur xterm.js), mais il va à l'encontre de l'objectif « client lourd natif ».

> Remarque : GPUI n'utilise pas non plus les widgets du système. Il dessine sa propre UI, mais en **code compilé directement sur le GPU**, sans moteur de navigateur entre les deux. C'est ce « natif » qu'on vise.

---

## 4. Stack technique

| Besoin | Choix | Notes |
|---|---|---|
| Framework UI | **GPUI** (Zed) | Rendu GPU, prouvé par Zed. Doc maigre, API mouvante : le code de Zed sert de référence. Alternative de repli : **iced** + `iced_term`. |
| Émulation terminal | **`alacritty_terminal`** | Utilisé par Zed (`crates/terminal`, `crates/terminal_view`). |
| PTY | **`alacritty_terminal::tty`** | Retenu à la place de `portable-pty` : déjà relié à l'émulateur et à sa boucle d'E/S, comme dans Zed. |
| Git (lecture) | **`gix`** | Statut, branches, HEAD, diff. |
| Git (worktrees, opérations) | **CLI `git`** en sous-processus | `git worktree` est mal couvert par les libs. |
| GitHub | **`octocrab`** (REST + GraphQL) | Phase de démarrage possible via la CLI `gh` pour réutiliser son authentification. GitHub Projects v2 = GraphQL uniquement. |
| Surveillance de fichiers | **`notify`** | `.git/HEAD`, refs, transcripts. |
| Sérialisation | **`serde`**, `serde_json`, `toml` | |
| Persistance d'état | **JSON** (`state.json`) | Retenu pour la v1 : l'état se limite à la disposition. SQLite reste possible pour l'historique et les statistiques. |
| Async | À trancher | GPUI a son propre exécuteur (Zed utilise surtout `smol`). Une cohabitation avec `tokio` est à valider dans le spike. |

### Découpage en crates (workspace Cargo)

```
grove/
├── crates/
│   ├── grove-app/        # binaire GPUI : fenêtres, sidebar, onglets, vues
│   ├── grove-core/       # modèle de données, état, persistance, config
│   ├── grove-term/       # PTY + alacritty_terminal + rendu grille
│   ├── grove-claude/     # serveur d'événements hooks, statusline, transcripts, stream-json
│   ├── grove-git/        # gix + wrapper CLI git, worktrees
│   ├── grove-github/     # octocrab, PR, CI, Projects
│   └── (grove-hook)      # remplacé par les sous-commandes `grove hook` / `grove statusline`
```

---

## 5. Modèle de données

```rust
struct Project {
    id: ProjectId,
    name: String,
    root: PathBuf,               // dossier principal (repo git ou non)
    git: Option<GitInfo>,        // None = dossier non-git : features git désactivées
    workspaces: Vec<Workspace>,
    config: ProjectConfig,       // scripts de setup, emplacement des worktrees…
}

struct GitInfo {
    common_dir: PathBuf,         // `git rev-parse --git-common-dir`, sert à regrouper
    default_branch: String,
    remote: Option<GitRemote>,   // pour l'intégration GitHub
}

struct Workspace {
    id: WorkspaceId,
    path: PathBuf,
    kind: WorkspaceKind,
    tabs: Vec<Tab>,
}

enum WorkspaceKind {
    /// Checkout principal : on travaille directement dans le dossier, branche courante.
    Direct,
    /// git worktree dédié.
    Worktree { branch: String, base: String },
}

enum Tab {
    Terminal { pty: PtyId, title: String },
    Claude(ClaudeTab),
}

struct ClaudeTab {
    session_id: Option<String>,  // connu après SessionStart
    view: ClaudeView,
    status: ClaudeStatus,
    metrics: Option<SessionMetrics>, // modèle, effort, coût, contexte (statusline / stream-json)
}

/// Limites du compte claude.ai, communes à toutes les sessions (statusline `rate_limits`).
struct AccountUsage {
    five_hour: Option<RateWindow>,        // fenêtre glissante de 5 h
    seven_day: Option<RateWindow>,        // semaine, tous modèles
    seven_day_opus: Option<RateWindow>,
    seven_day_sonnet: Option<RateWindow>,
    received_at: SystemTime,              // fraîcheur de la donnée
}

struct RateWindow {
    used_percent: f64,
    resets_at: Option<i64>,               // epoch Unix, en secondes
}

enum ClaudeView {
    /// Phase 1 : CLI `claude` dans un PTY (TUI officiel).
    Terminal,
    /// Phase 2 : interface native pilotant `claude -p` en stream-json.
    Native,
}

enum ClaudeStatus {
    Starting,
    Working { current_tool: Option<String> },
    NeedsInput { reason: NeedsInputReason }, // permission, question, inactif
    Idle,
    Exited { code: Option<i32> },
}
```

### Rendu attendu de la sidebar

```
▾ rgsupv-dashboard
    ⌂ main                            2
    ⎇ feat-1234-tree         ● attend
    ✳ fix-987-login          ● en cours
▾ notes-perso                   pas git
    ▭ dossier
```

Icône de tête : maison pour le checkout principal (direct), branche pour un
worktree, ✳ pour un worktree créé par Claude Code, dossier hors git ; une
infobulle sur l'icône rappelle le type et le chemin. À droite : l'état Claude
agrégé (pastille ambre « attend », verte « en cours »), sinon le nombre
d'onglets s'il y en a plusieurs. La sidebar se redimensionne par son bord
droit (double-clic : largeur par défaut) ; sa largeur est conservée dans
`state.json`. Les projets se réordonnent par glisser-déposer de leur ligne :
déposé sur un autre projet (sa ligne ou ses espaces), le projet glissé prend
sa place. L'ordre est celui de `state.json`.

---

## 6. Feuille de route

| Phase | Contenu | Livrable |
|---|---|---|
| **0 — Spike** | Fenêtre GPUI + un terminal PTY fonctionnel | Valider GPUI + alacritty_terminal + runtime async |
| **1 — Socle** | Projets, espaces de travail **directs**, onglets terminal, persistance, branches en mode direct (§7.4) | Remplace tmux ou les onglets au quotidien |
| **2 — Claude, mode terminal** | Onglet Claude = `claude` dans un PTY, hooks, badges d'état, notifications, métriques | Pilotage multi-sessions |
| **3 — Worktrees** | Création, setup, promotion, nettoyage des worktrees | Travail parallèle isolé |
| **4 — Claude, interface native** | Vue native stream-json, bascule terminal ↔ native | « Petite interface » Claude |
| **5 — Profils et paramètres** | Profils de terminal (SSH préconfigurés…), page Paramètres, liste des raccourcis | Terminaux distants et réglages sans éditer le TOML |
| **6 — GitHub PR / CI** | PR liée à la branche, statut CI, review | |
| **7 — GitHub Projects** | Tableau, liaison ticket ↔ worktree | |
| **8 — Recette des stories** | Claude joue la procédure de test d'une story | Recette assistée, compte-rendu sur le ticket |

Les phases 3 et 4 sont indépendantes et peuvent être interverties. La phase 5 ne dépend que du socle : elle peut passer avant ou après la 4. La phase 8 suppose la liaison ticket ↔ espace de travail (phase 7).

---

## 7. Phase 0–1 : terminal et socle

### 7.1 Terminal

Exigences minimales pour un usage quotidien (et pour faire tourner le TUI de Claude Code correctement) :

- couleurs 24 bits, attributs (gras, italique, souligné, inverse) ;
- redimensionnement (SIGWINCH) ;
- scrollback configurable, recherche dans le scrollback ;
- sélection souris, copier/coller, **bracketed paste** ;
- reporting souris (pour les TUI) ;
- saisie clavier complète, y compris les combinaisons Alt/Ctrl et l'IME (texte composé reçu par un `InputHandler` GPUI : sous IBus, une lettre accentuée n'arrive pas comme une frappe) ;
- liens cliquables (OSC 8 + détection d'URL) : `Ctrl+clic`, soulignés au survol avec `Ctrl`, schémas connus seulement ;
- **OSC 7** (cwd courant) et titre de fenêtre (OSC 0/2) ;
- barre de progression (**OSC 9;4**, celle de ConEmu et Windows Terminal) : trait au pied de l'onglet, couleur d'accent, rouge en erreur, ambre en pause, segment qui va et vient si l'avancement est inconnu. Claude Code ne l'émet que pour ConEmu, Ghostty et iTerm2 : Grove définit `ConEmuTask` dans ses terminaux ;
- performances : pas de lag sur une sortie massive (`cat` d'un gros fichier, redraws de Claude).

> Risque n°1 du projet : l'émulation terminal concentre environ la moitié de l'effort. S'appuyer au maximum sur `crates/terminal` et `crates/terminal_view` de Zed.

### 7.2 Projets et espaces de travail

- Ajouter un projet = choisir un dossier. Détection git automatique.
- Chaque projet a au minimum un espace de travail **Direct** (le dossier lui-même).
- Les dossiers non-git sont supportés : les fonctionnalités git sont simplement masquées.
- Sidebar : projets repliables et réordonnables (glisser-déposer) → espaces de travail → nombre d'onglets et état.
- Onglets par espace de travail : nouveau terminal (cwd = chemin de l'espace de travail), fermer, renommer, réordonner.
- Affichage de la branche courante en mode direct, **mise à jour en direct** via la surveillance de `.git/HEAD`.

### 7.3 Persistance

- Restauration au démarrage : projets, espaces de travail, disposition des onglets, onglet actif.
- Les terminaux sont relancés (pas de restauration du contenu en v1).
- Fichiers :
  - `~/.config/grove/config.toml` : configuration globale (thème, police, raccourcis) ;
  - `~/.local/share/grove/` : état (projets, onglets, sessions) ;
  - `<projet>/.grove.toml` (optionnel, versionnable) : config projet (scripts de setup, emplacement des worktrees).

### 7.4 Branches (mode direct)

Gestion des branches du checkout principal, sans worktree. Les worktrees (phase 3) réutilisent les mêmes briques.

- **Pastille de branche** à gauche de la barre d'onglets : `⎇ main ↑1 ↓2 ●3`, soit avance et retard sur la branche suivie, puis fichiers modifiés (indexés, non indexés ou non suivis).
  - Branche : surveillance de `.git/HEAD`, mise à jour immédiate.
  - Compteurs : `git status --porcelain=v2 --branch`, relancé au changement de `HEAD`, après chaque opération et toutes les 15 s, hors du thread de l'UI.
- **Sélecteur de branches** (clic sur la pastille ou `Ctrl+Shift+G`), affiché par-dessus le terminal :
  - champ de recherche : filtre au fil de la frappe, `↑`/`↓` pour naviguer, `Entrée` pour valider, `Échap` pour fermer ;
  - branches locales puis distantes, de la plus récemment modifiée à la plus ancienne (`git for-each-ref --sort=-committerdate`), branche courante cochée, branche suivie affichée ;
  - **switch** : `git switch <branche>` ; une branche distante `origin/x` crée la branche locale `x` qui la suit ;
  - **création** : si la recherche ne correspond à aucune branche locale, entrée « Créer « x » depuis la branche courante » (`git switch -c`, nom validé par `git check-ref-format`) ;
  - **suppression** d'une branche locale autre que la courante (bouton au survol) : `git branch -d` après confirmation ; si git refuse parce qu'elle n'est pas fusionnée, seconde confirmation explicite (« ses commits seront perdus ») puis `git branch -D` ;
  - erreurs de git affichées telles quelles dans le sélecteur.
- **Garde-fous avant un switch**, confirmés dans le sélecteur :
  - fichiers modifiés : ils suivent le changement de branche, et git refuse en cas de conflit ;
  - session Claude active (en cours ou en attente) dans l'espace : elle verrait les fichiers changer sous elle.
- Les opérations passent par la CLI `git` (messages forcés en anglais, `LC_ALL=C`, pour reconnaître les erreurs), hors du thread de l'UI.
- Plus tard :
  - `git fetch` à la demande et en tâche de fond ;
  - push et pull depuis la pastille ;
  - stash avant un switch ;
  - suggestion « créer un worktree plutôt » quand une session Claude tourne dans l'espace (phase 3).

---

## 8. Phase 2 : Claude Code en mode terminal

Un onglet Claude = la CLI `claude` lancée dans un PTY, dans le dossier de l'espace de travail. L'utilisateur voit le TUI officiel, avec toutes ses fonctionnalités (slash commands, plan mode, `/compact`…).

### 8.1 Remontée d'informations via les hooks

L'app injecte sa propre configuration **sans toucher aux settings de l'utilisateur**, via le flag `--settings <fichier>` (à vérifier dans la version courante de Claude Code) :

```
claude --settings ~/.local/share/grove/claude-settings.json [--resume <id>]
```

Variables d'environnement passées au PTY :

- `GROVE_SOCKET` : chemin du socket Unix de l'app ;
- `GROVE_TAB_ID` : identifiant de l'onglet.

Les hooks appellent `grove hook` (sous-commande du binaire principal, traitée avant toute initialisation de l'UI : pas de second binaire à installer ni à garder synchronisé). Elle lit le JSON reçu sur stdin, ajoute `GROVE_TAB_ID` et transmet le tout au socket. Elle doit être **rapide et ne jamais bloquer** Claude : timeout court, et en cas d'échec elle abandonne en silence. Elle n'écrit **rien sur stdout** : pour `SessionStart` et `UserPromptSubmit`, Claude Code ajouterait cette sortie au contexte.

Correspondance événement → état :

| Hook | Effet |
|---|---|
| `SessionStart` | Récupère `session_id`, statut `Idle` |
| `UserPromptSubmit` | `Working` |
| `PreToolUse` | `Working { current_tool }` |
| `PostToolUse` | Mise à jour de l'activité |
| `Notification` | `NeedsInput` (demande de permission, inactivité) → **notification système** |
| `Stop` | `Idle` (tour terminé) → notification si l'onglet n'est pas visible |
| `SessionEnd` | `Exited` |

### 8.2 Métriques via la statusline

- L'app configure une commande `statusLine` (dans le même fichier `--settings`) qui reçoit le JSON de la statusline : modèle (`model`), effort (`effort.level`), coût (`cost`), utilisation du contexte (`context_window`), limites du compte (`rate_limits`), `session_id`, cwd.
- Le binaire transmet ces données à l'app **et** affiche une ligne dans le TUI.
- Si l'utilisateur a déjà sa propre statusline, `grove statusline` l'appelle en chaîne et renvoie sa sortie, pour ne pas la casser (options `padding` et `refreshInterval` reprises).

### 8.3 Fonctionnalités UI

- Badge d'état par onglet et par espace de travail dans la sidebar (agrégé : « 1 session attend »).
- Notifications système (crate `notify-rust`) avec action « aller à l'onglet ».
- Panneau latéral de session : modèle et **niveau d'effort**, coût cumulé, jauge de contexte, outil en cours, lignes ajoutées et supprimées.
- Liste des sessions passées d'un espace de travail (lues depuis les transcripts) → **reprendre** (`claude --resume <id>`).
- Raccourci « aller à la prochaine session qui attend une réponse ».

### 8.4 Transcripts

- Emplacement : `~/.claude/projects/<cwd encodé>/<session_id>.jsonl`.
- Usage : historique, recherche, statistiques d'usage.
- Titre d'une session : dernière entrée `custom-title` (`/rename`), sinon `ai-title` (généré), sinon premier prompt. Coût : dernière entrée `cost-state`. Les fichiers peuvent dépasser 20 Mo : filtrage des lignes par sous-chaîne avant parsing, lecture hors du thread de l'UI.
- Lecture **défensive** : format interne, champs inconnus ignorés, aucune erreur bloquante.
- Les worktrees apparaissent comme des dossiers distincts : on les regroupe par repo via `GitInfo.common_dir`.

### 8.5 Usage du compte (limites 5 h et hebdomadaire)

Source : champ `rate_limits` de la statusline (`five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`, chacun avec `used_percentage` et `resets_at`). Ce sont des limites de **compte**, communes à toutes les sessions. Une session inactive renvoie pourtant, à chaque rafraîchissement de sa statusline, les valeurs de sa dernière réponse : les valeurs reçues sont donc **fusionnées par fenêtre**. Une date de remise à zéro plus tardive désigne une fenêtre plus récente ; dans une même fenêtre (à 60 s près), l'usage ne fait que monter et le plus haut l'emporte. Absent en connexion par clé d'API : le widget est alors masqué.

Claude Code ne transmet `rate_limits` qu'**après la première réponse** d'une session (vérifié en 2.1.306 : le JSON envoyé au démarrage ne contient pas la clé). Pour afficher la carte dès le lancement :

- les dernières limites connues sont **persistées** dans `~/.local/share/grove/usage.json` (écrit quand elles changent, et au moins toutes les 5 min pour que leur âge reste juste) et rechargées au démarrage, avec leur âge (« mis à jour il y a 40 min ») ;
- les dates de remise à zéro sont absolues : le compte à rebours reste juste avec des valeurs anciennes ;
- une fenêtre dont la date de remise à zéro est passée s'affiche « réinitialisée », sans pourcentage périmé ;
- le seuil d'alerte déjà notifié est persisté aussi, pour ne pas renotifier après un redémarrage.

**Option, désactivée par défaut : API d'usage** (`[claude] usage_api = true`). Grove appelle l'endpoint d'usage d'Anthropic avec le jeton OAuth de Claude Code, comme le fait ccstatusline :

- requête `GET https://api.anthropic.com/api/oauth/usage`, en-têtes `Authorization: Bearer <accessToken>` et `anthropic-beta: oauth-2025-04-20` ;
- réponse : fenêtres `{ utilization, resets_at }`, avec la liste `limits[]` en repli ;
- le jeton est lu dans `~/.claude/.credentials.json`. Il n'est jamais rafraîchi ni réécrit, et n'est envoyé qu'à `api.anthropic.com`. S'il a expiré, l'appel est sauté : Claude Code le renouvellera ;
- au démarrage, puis toutes les 3 min, **sauf** si la statusline a fourni des limites depuis moins de 3 min. Des sessions tournent alors et l'appel serait redondant ; ccstatusline interroge d'ailleurs le même endpoint avec le même jeton, et la limite d'appels est commune (constaté : `Retry-After: 30` pour les deux outils) ;
- après un refus pour excès d'appels (HTTP 429) : attente de `Retry-After` (30 s au minimum), sinon 30 s, 60 s, 120 s puis 300 s pour les refus suivants ;
- en attendant, la carte affiche les dernières valeurs persistées, avec leur âge ;
- même traitement que les valeurs de la statusline : persistance, alertes, affichage.

⚠ **Hors des points d'intégration officiels** (principe 2) : endpoint interne non documenté, qui peut changer sans préavis. Surtout, réutiliser les identifiants claude.ai dans un outil tiers va a priori contre les conditions d'Anthropic, et le risque porte sur le compte de l'utilisateur. C'est pour cela que l'option est désactivée par défaut et doit être activée explicitement. Lire le cache de ccstatusline est écarté : il repose sur le même appel, sans contrôle de Grove.

- **Carte compacte en bas à gauche** (pied de la sidebar) :
  - anneau de progression de la fenêtre de 5 h, pourcentage au centre ;
  - compte à rebours avant la remise à zéro (« reset dans 2 h 14 »), rafraîchi toutes les 30 s ;
  - pourcentage hebdomadaire en petit.
- **Clic : détail déplié** au-dessus de la carte :
  - une barre par fenêtre (5 h, semaine tous modèles, semaine Opus, semaine Sonnet si présentes) ;
  - date de remise à zéro en heure locale (« lun. 27 oct. à 14:00 ») et compte à rebours ;
  - **repère de rythme** : trait sur la barre à la part de la fenêtre déjà écoulée, pour voir d'un coup d'œil si la consommation va plus vite que le temps ;
  - fraîcheur de la donnée (« mis à jour il y a 12 min » quand aucune session n'a tourné récemment).
- Couleurs par seuil : vert, ambre à partir de 70 %, rouge à partir de 90 %.
- **Notification système** quand la fenêtre de 5 h franchit 90 % puis 100 %, une fois par seuil et par fenêtre.
- Plus tard : historique de consommation par jour et par projet (cf. §8.6).

### 8.6 Optionnel

- Export OpenTelemetry de Claude Code vers un petit collecteur intégré, pour des statistiques d'usage agrégées (tokens, coût par projet et par jour).

---

## 9. Phase 3 : Worktrees (optionnels)

Les deux modes coexistent dans un même projet : l'espace **Direct** est toujours présent, et les worktrees s'y ajoutent.

### 9.1 Création

- Action « Nouvel espace de travail » : choix de la branche (nouvelle ou existante) et de la base.
- Emplacement configurable par projet :
  - `../<repo>.worktrees/<branche>` (par défaut), ou
  - `<repo>/.worktrees/<branche>` (ajouté à `.git/info/exclude`).
- **Script de setup** exécuté après la création, défini dans `.grove.toml`, avec sortie visible dans un onglet :

```toml
[worktree]
location = "sibling"          # "sibling" | "inside"
setup = [
  "cp ../{{repo}}/.env .env",
  "npm install",
]
copy = [".env", ".env.local"] # raccourci : fichiers copiés depuis le checkout principal
```

- Option « ouvrir une session Claude dans le nouveau worktree » directement.
- **Fenêtre « Nouvel espace de travail »** (`Ctrl+Shift+N`, ou `+` au survol du projet dans la sidebar) :
  - un champ : le nom tapé propose « Créer « x » depuis « main » » et « … depuis « <branche courante> » » (branche par défaut parmi `main`, `master`, `develop`) ;
  - le même champ filtre les branches existantes : locales non extraites ailleurs (git refuse d'extraire deux fois une branche), et distantes sans équivalent local (création d'une branche locale qui suit) ;
  - `Entrée` crée l'espace avec un terminal, `Maj+Entrée` avec une session Claude ;
  - pied de fenêtre : emplacement des worktrees et nombre de commandes de setup.
- Après la création :
  - copie des fichiers `copy` depuis le checkout principal ;
  - si `setup` est défini, onglet « setup » qui enchaîne les commandes, affiche leur code de sortie puis laisse un shell ;
  - l'espace est activé.
- Création hors du thread de l'UI. Erreurs de git affichées dans la fenêtre, refus propre si le dossier existe déjà.

### 9.2 Mode direct : garde-fous

- Changement de branche détecté (`.git/HEAD`) → mise à jour de l'UI et **avertissement** si une session Claude est en cours dans cet espace.
- Plusieurs sessions Claude dans le même espace direct : autorisé, avec avertissement et suggestion « créer un worktree ».
- Les avertissements s'affichent en bandeau sous la barre d'onglets, avec un bouton « Créer un worktree » et une croix pour fermer. Un changement de branche lancé depuis le sélecteur de Grove n'en déclenche pas : il a déjà été confirmé (§7.4).

### 9.3 Promotion direct → worktree

Action « déplacer ce travail dans un worktree » :

1. `git stash` (y compris les fichiers non suivis) dans l'espace direct ;
2. création du worktree sur une nouvelle branche ;
3. `git stash pop` dans le worktree ;
4. option de reprendre la session Claude dans le nouvel espace (**pas encore fait** : à vérifier d'abord que `claude --resume <id>` retrouve une session lancée depuis un autre dossier, les transcripts étant rangés par cwd).

- Accès : entrée « Déplacer le travail en cours dans un worktree… » en bas du sélecteur de branches d'un espace direct. La nouvelle branche part de `HEAD`.
- En cas d'échec de la création, le stash est réappliqué dans l'espace direct. Si c'est la réapplication dans le worktree qui échoue, l'erreur le dit : les modifications restent dans `git stash list`.

### 9.4 Nettoyage

- Suppression d'un worktree (`git worktree remove`) avec confirmation s'il reste des modifications.
- Option de supprimer la branche locale, voire distante.
- Fenêtre de suppression (`×` au survol d'un worktree dans la sidebar) :
  - **Retirer de Grove** : le dossier reste. Le chemin est mémorisé (`ignored_worktrees`) pour que l'import ne le fasse pas revenir ;
  - **Supprimer le worktree** (`--force` après avertissement s'il reste des modifications) ;
  - **Supprimer worktree et branche** : `git branch -d` ; une branche non fusionnée est conservée, avec un message ;
  - suppression de la branche distante : plus tard.
- Phase 6 : suggestion automatique de nettoyage quand la PR associée est mergée.
- Détection des worktrees créés hors de l'app (`git worktree list`) → import dans la sidebar, au démarrage et à l'ajout d'un projet. Cela inclut les worktrees de Claude Code (`.claude/worktrees/`). Les worktrees dont le dossier a disparu (`prunable`) sont ignorés.

### 9.5 Infos git par espace de travail

- Branche, avance/retard par rapport à la base et au remote, nombre de fichiers modifiés : pastille de la barre d'onglets (§7.4), commune aux espaces directs et worktrees. L'avance et le retard sont calculés par rapport à la branche suivie ; par rapport à la base : plus tard.
- Vue diff simple (fichiers modifiés → diff) : plus tard.

---

## 10. Phase 4 : interface native Claude

Une « petite interface » qui remplace le TUI pour l'usage courant, **sans supprimer le mode terminal**.

### 10.1 Pilotage de la CLI

```
claude -p --input-format stream-json --output-format stream-json \
       --include-partial-messages --verbose \
       --settings ~/.local/share/grove/claude-settings.json \
       [--resume <session_id>]
```

- stdout : flux d'événements JSON (init avec `session_id`, messages assistant, deltas de streaming, `tool_use`, `tool_result`, résultat final avec coût et usage).
- stdin : messages utilisateur.
- Types `serde` tolérants (`#[serde(other)]`, champs optionnels) pour résister aux évolutions.

Avec `--include-partial-messages`, chaque bloc arrive en morceaux (`stream_event`) puis en entier dans un message `assistant` (un message par bloc, vérifié en 2.1.308) : le texte vient des morceaux, l'entrée des outils du message complet. Les lignes non JSON (bannière d'un alias du shell) sont ignorées. `--resume` ne renvoie pas l'historique : la vue le relit dans le transcript.

### 10.2 Permissions

- **Approche principale (Rust pur)** : `--permission-prompt-tool` pointant vers un **serveur MCP exposé par l'app**. Chaque demande s'affiche dans l'UI : Accepter / Refuser / Toujours accepter.
  - Fait : `grove mcp-permission`, déclaré par `--mcp-config` avec `GROVE_SOCKET` et `GROVE_TAB_ID`, transmet `{ tool_name, input, tool_use_id }` au socket (message `permission`) et attend la décision sur la même connexion (`{"behavior": "allow", "updatedInput"}` ou `{"behavior": "deny", "message"}`). Connexion fermée sans réponse = refus. « Toujours » vaut pour l'outil, jusqu'à la fin de la session.
  - Les actions classées sensibles par Claude Code (fichiers de `~/.claude`…) sont refusées sans consulter l'outil.
- **Repli** : si des fonctions du protocole de contrôle des SDK officiels deviennent nécessaires (interruption, changement de mode, `canUseTool`), passer par un **sidecar Node** qui embarque le SDK TypeScript et dialogue avec l'app en JSON-RPC. Ce protocole n'est pas une API publique stable, d'où le sidecar plutôt qu'une réimplémentation en Rust.

### 10.3 Fonctionnalités UI

- Fil de conversation : markdown, coloration syntaxique, images.
- Appels d'outils repliables (commande Bash et sa sortie, lecture de fichier, recherche…).
- **Diffs riches** pour `Edit` / `Write` / `MultiEdit`, avec Accepter / Refuser.
- Streaming en temps réel.
- Jauge de contexte et coût en direct.
- Champ de saisie : multiligne, historique, `@fichier` avec autocomplétion, glisser-déposer d'images.
- Bouton d'interruption.

État (première itération) :

- fait : streaming, markdown simple (titres, listes, code, citations, gras, code en ligne), appels d'outils repliables avec résultat (40 lignes), différence `old_string` / `new_string` dans le détail et dans la demande de permission, copie d'une réponse, historique des prompts, coût cumulé et tokens de contexte, mode de permission (`Maj+Tab`), liens cliquables (`[texte](url)` et URL nues, `http(s)` seulement), résumé de compactage replié (`isCompactSummary`) au lieu d'un message, **sélecteurs de modèle et d'effort** dans le pied du champ de saisie (modèle : défaut, Fable, Opus, Sonnet, Haiku, `--model` ; effort : défaut, low à max, `--effort`), appliqués au prochain message par relance avec `--resume`, comme le mode de permission. Valeurs affichées : celle choisie, sinon celle de la session (modèle de chaque réponse, relu aussi dans le transcript ; effort de l'init si Claude Code le transmet), sinon celle des réglages de Claude Code (`model`, `effortLevel` ; projet puis utilisateur) ;
- markdown : titres hiérarchisés, italique (hors des mots : `snake_case` et `2*3*4` restent tels quels), barré, cases à cocher, citations sur plusieurs lignes, images (texte alternatif en lien), **tableaux** (alignements GFM, colonnes proportionnelles au contenu, cellules qui passent à la ligne) ;
- fil **virtualisé** (`list` de GPUI, alignée en bas : elle suit les nouveaux messages tant qu'on n'a pas remonté) : seuls les messages visibles sont construits et mesurés. Une empreinte légère par entrée (type, longueurs, résultat, dépliée ou non) dit lesquelles remesurer. Mesuré sur une session de 480 textes : 49 ms par image en défilement avant, 3 ms après ;
- chaque message est mis en page en blocs empilés, pas en colonne flex : la colonne flex de GPUI réservait des hauteurs trop petites aux longs textes qui passent à la ligne, et les messages se chevauchaient ;
- interruption : `SIGINT` au groupe du processus, la session reprend au message suivant (`--resume`). L'interruption sans arrêt passerait par le protocole de contrôle (repli du §10.2) ;
- à faire : modèle, effort et mode mémorisés dans l'onglet, coloration syntaxique, images, `@fichier`, glisser-déposer, jauge de contexte en pourcentage (la taille de la fenêtre n'est pas dans le flux), sélection du texte du fil, limites d'usage depuis `rate_limit_event` (unité de `utilization` à vérifier).

### 10.4 Bascule de vue

- La session est l'entité, la vue (`Terminal` ou `Native`) est un attribut de l'onglet.
- Basculer = arrêter le processus courant, puis relancer avec `--resume <session_id>` dans l'autre mode. Les deux partagent le même transcript.
- **Une seule vue active à la fois par session** (verrou côté app) : la bascule (`Ctrl+Shift+M`) détruit la vue courante et son processus avant de créer l'autre. La vue est persistée dans l'onglet (`TabKind::Claude { view }`), `[claude] view` choisit celle des nouveaux onglets.
- Usage type : vue native au quotidien, bascule vers le terminal pour une fonctionnalité pas encore gérée (slash command spécifique, plan mode…).

### 10.5 Limites assumées

- Parité jamais totale avec le TUI officiel : c'est le rôle du mode terminal.
- Certaines commandes interactives n'existent pas en headless.
- En cas de distribution publique, vérifier les conditions d'utilisation liées à la connexion par abonnement claude.ai dans un outil tiers.

---

## 11. Phase 5 : profils de terminal et paramètres

### 11.1 Profils de terminal

Aujourd'hui un onglet terminal lance toujours le shell (`[terminal] shell`, sinon `$SHELL`). Un **profil** décrit ce qu'un onglet lance :

- nom, icône, et la commande : shell local (défaut), **SSH**, ou commande libre (`docker exec -it app bash`, `kubectl exec…`, `psql`…) ;
- dossier de départ (par défaut : l'espace de travail), variables d'environnement ;
- profils **globaux** (`config.toml`) et **par projet** (`.grove.toml`, versionnable : le profil « staging » d'un repo est partagé avec l'équipe).

```toml
[[terminal.profiles]]
name = "staging"
kind = "ssh"
host = "staging-web-1"           # alias de ~/.ssh/config ou nom d'hôte
user = "deploy"                  # optionnel, comme port, identity, jump
remote_command = "cd /var/www/app && exec $SHELL -l"

[[terminal.profiles]]
name = "db"
command = "docker compose exec db psql -U app"
```

#### SSH

- Grove lance la **CLI `ssh` du système** dans le PTY, sans bibliothèque SSH embarquée : `~/.ssh/config`, l'agent, les clés, `known_hosts` et `ProxyJump` fonctionnent comme dans un terminal ordinaire.
- **Profils préfaits** : les entrées `Host` de `~/.ssh/config` (hors motifs `*`, `?`) sont proposées comme profils prêts à l'emploi, sans les recopier ; un profil Grove peut en surcharger un (utilisateur, commande distante).
- Champs d'un profil SSH : hôte, utilisateur, port, clé (`-i`), rebond (`-J`), options `-o` (`ServerAliveInterval`…), commande distante (`-t` ajouté).
- Grove **ne stocke aucun mot de passe** ni passphrase : `ssh` les demande dans le terminal.
- Onglet SSH : icône serveur, titre = nom du profil. L'OSC 7 renvoyé par le shell distant désigne un dossier distant : il n'est pas repris comme dossier de départ des onglets locaux.
- Connexion fermée : bandeau « Connexion fermée (code n) » avec **Reconnecter**.
- Au redémarrage, les onglets SSH sont restaurés **sans se connecter** (bouton « Se connecter ») : pas de rafale de connexions ni de demandes de passphrase ; option `reconnect = "auto"` par profil.

#### Ouvrir un onglet avec un profil

- `Ctrl+Shift+T` et le `+` de la barre d'onglets : profil par défaut (`[terminal] default_profile`, sinon le shell).
- Flèche à côté du `+` (ou clic droit) : liste des profils, avec recherche, comme le sélecteur de branches.
- L'onglet garde la référence de son profil (`TabKind::Terminal { profile: Option<String> }`), persistée dans `state.json`.

### 11.2 Page Paramètres

- Ouverte par `Ctrl+,` ou une icône d'engrenage en pied de sidebar ; elle remplace la zone des onglets (la sidebar reste visible), `Échap` la ferme.
- Sections à gauche, contenu à droite :
  - **Général** : police, taille, thème (plus tard) ;
  - **Terminal** : shell, scrollback, profil par défaut ;
  - **Profils** : liste, création, édition, duplication, profils importés de `~/.ssh/config` (lecture seule, « personnaliser » en crée une copie) ;
  - **Claude** : commande, vue par défaut, notifications, API d'usage (avec l'avertissement du §8.5), skills de review (phase 6) ;
  - **Raccourcis clavier** (§11.3).
- `config.toml` reste la **source de vérité** : la page l'écrit avec `toml_edit` pour garder commentaires et mise en forme, et un bouton l'ouvre dans l'éditeur. Le fichier est surveillé (`notify`) : une modification à la main s'applique sans redémarrer.
- Un réglage de projet (`.grove.toml`) s'affiche avec sa provenance ; la page ne l'écrit pas dans le repo sans le dire.

### 11.3 Raccourcis clavier

- Liste de tous les raccourcis, groupés (onglets, espaces de travail, branches, Claude, sidebar, terminal), avec recherche par action ou par touche.
- Générée depuis les `KeyBinding` enregistrés : une seule source, la liste ne peut pas diverger du comportement.
- Accessible depuis les Paramètres ; un raccourci direct reste à choisir, sans masquer une touche utile aux programmes du terminal (`Ctrl+/` envoie `^_`, utilisé par nano ou emacs).
- Plus tard : personnalisation (`[keys]` dans `config.toml`), avec détection des conflits.

---

## 12. Phases 6–7 : GitHub

### 12.1 Pull requests et CI (phase 6)

- Détection du remote GitHub depuis `GitInfo.remote`.
- Par espace de travail : PR associée à la branche (état, review, checks).
- Créer une PR depuis l'espace de travail (titre et corps éventuellement générés par Claude).
- Statut CI en direct (polling, ou webhook plus tard), notification quand un check échoue.
- Lecture des commentaires de review, avec envoi d'un commentaire vers la session Claude (« corrige ce point »).

#### Reviews assistées par Claude

Deux listes, dans un panneau « Pull requests » (filtrées par projet, ou globales) :

- **PR où je suis reviewer** (review demandée ou déjà donnée, `review-requested:@me`) :
  - action **« Reviewer »** : ouvre un onglet Claude dans l'espace de travail de la branche de la PR (worktree créé si besoin, phase 3 ; sinon checkout dans l'espace direct après confirmation), et lance un **skill existant** configurable, avec l'URL de la PR en argument ;
  - la session est **liée à la PR** (numéro de PR ↔ `session_id`, persisté) ;
  - action **« Actualiser la review »**, mise en avant quand l'auteur a répondu ou poussé depuis la dernière review (nouveaux commits, réponses aux fils) : reprend la **même session** (`--resume`) avec un prompt qui résume ce qui a changé ;
  - badge sur la PR : « à reviewer », « review envoyée », « nouvelles réponses ».
- **PR dont je suis l'auteur** :
  - liste des commentaires de review non résolus, groupés par fil ;
  - action **« Prendre en compte les commentaires »** : ouvre ou reprend la session Claude liée à la branche, avec un skill configurable et les commentaires en contexte (fichier, ligne, texte, auteur) ;
  - après la correction : brouillon de réponse par fil, envoyé et fil résolu **seulement après validation** ;
  - badge : « changements demandés », « n commentaires à traiter ».
- Configuration (globale dans `config.toml`, surchargeable dans `.grove.toml`) :

```toml
[github.review]
skill = "/morty:review"          # skill lancé pour reviewer une PR ({{pr_url}} ajoutée en argument)
refresh_prompt = "L'auteur a répondu ou poussé de nouveaux commits, mets la review à jour."

[github.address]
skill = "/morty:implement"       # skill lancé pour traiter les commentaires reçus
```

- Détection des changements : polling léger (ETag) des PR suivies, notification « nouvelles réponses sur la PR #123 ».
- Authentification : `gh auth token` dans un premier temps, OAuth device flow ensuite.

### 12.2 GitHub Projects (phase 7)

- Vue tableau d'un Project v2 (GraphQL).
- Depuis un ticket : créer le worktree et la branche (convention de nommage configurable, ex. `feat-<issue>-<slug>`) et ouvrir une session Claude avec le ticket en contexte.
- Mise à jour du statut du ticket selon l'état de la PR.

---

## 13. Phase 8 : recette des stories

Faire tester une story par Claude en suivant la **procédure de test** écrite dans le ticket.

- **Source** : la story liée à l'espace de travail (phase 7) ou choisie dans le tableau. La procédure est la section dont le titre correspond à l'un de ceux configurés (« Procédure de test », « Comment tester », « Test plan »…), dans le ticket puis dans la PR ; à défaut, le corps entier, en le signalant.
- **Action « Tester la story »** : ouvre un onglet Claude dans l'espace de la branche (worktree créé si besoin, phase 3) et lance un skill configurable avec l'URL du ticket, la procédure et les critères d'acceptation en contexte.
- **Environnement** : commandes de préparation de `.grove.toml` (`[test] setup`, ex. démarrer le serveur de dev) dans un onglet dédié avant la session ; les profils de terminal (§11.1, ex. SSH vers la recette) sont utilisables par la procédure.
- **Compte-rendu par étape** : réussie, échouée, bloquée, non jouée, avec les preuves (sortie de commande, capture d'écran si un navigateur est piloté). Affiché dans un panneau de la story, persisté avec le lien story ↔ session.
- **Relancer le test** après de nouveaux commits : reprend la même session (`--resume`) avec ce qui a changé.
- **Publication** : commentaire sur le ticket ou la PR, et passage éventuel au statut suivant (« Testé »), **seulement après validation**.
- **Recette manuelle** : la procédure s'affiche aussi en liste à cocher, pour les étapes que l'utilisateur fait lui-même ; Claude peut être appelé sur une étape précise.
- **Garde-fous** :
  - le texte du ticket est transmis comme **données à tester**, pas comme des consignes : n'importe qui pouvant écrire dans le ticket, il ne doit pas élargir ce que la session a le droit de faire ;
  - les permissions de Claude Code restent actives (jamais `--dangerously-skip-permissions`) ;
  - une étape qui vise la production ou détruit des données demande une confirmation explicite.

```toml
[github.test]
skill = "/morty:qa"                                   # {{issue_url}} ajoutée en argument
sections = ["Procédure de test", "Comment tester", "Test plan"]

[test]                                                # .grove.toml
setup = ["docker compose up -d", "npm run dev"]
```

---

## 14. Questions ouvertes

- **Nom** du projet.
- ~~**GPUI vs iced**~~ : GPUI retenu (`gpui` 0.2.2 de crates.io, épinglé), validé par le spike.
- **Runtime async** : exécuteur de GPUI seul pour l'instant (threads dédiés pour le PTY, le socket et `notify`). `tokio` à réévaluer avec `octocrab` (phase 6).
- ~~**Persistance**~~ : JSON retenu pour la v1 (cf. §4).
- **Distribution Linux** : AppImage, `.deb`, AUR ? Flatpak compliqué (sandbox incompatible avec PTY, git et `claude` du système).
- **Restauration du contenu des terminaux** au redémarrage (sérialisation du scrollback) : v2 ?
- **Splits** (plusieurs terminaux côte à côte dans un onglet) : v1 ou plus tard ?

---

## 15. Hors périmètre (pour l'instant)

- Réimplémentation de la boucle d'agent avec l'API Anthropic.
- Support d'autres agents (Codex, Gemini CLI…). L'architecture ne doit pas l'empêcher, mais ce n'est pas un objectif.
- Windows.
- Éditeur de code intégré (au-delà de la vue diff).
- Collaboration multi-utilisateurs.

---

## 16. Risques

| Risque | Impact | Mitigation |
|---|---|---|
| Émulation terminal complexe | Élevé | S'appuyer sur `alacritty_terminal` et le code de Zed ; phase 0 dédiée |
| API GPUI instable, peu documentée | Moyen | Épingler une révision ; repli possible sur iced |
| Changements de format de Claude Code (JSONL, stream-json) | Moyen | Points d'intégration officiels d'abord, parsing tolérant |
| Dérive du périmètre | Élevé | Feuille de route stricte, chaque phase utilisable seule |
| Écosystème qui évolue vite (outils concurrents) | Moyen | Rester centré sur « natif, Linux, mon workflow » |
