//! Pull requests GitHub (specs §12.1) : celles où je suis reviewer et les
//! miennes, rafraîchies en tâche de fond ; panneau « Pull requests » et
//! sessions Claude lancées sur la branche d'une PR (worktree créé si besoin).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, FocusHandle, Hsla, KeyDownEvent, SharedString, Task, Window, div, prelude::*, px};
use grove_core::{ProjectConfig, ProjectId};
use grove_git::NewWorktree;
use grove_github::{AuthorState, CheckState, FetchError, Inbox, PullRequest, ReviewState, Thread, Token, TokenSource};

use crate::grove_view::{GroveView, WorkspaceKey};
use crate::marquee::marquee;
use crate::settings::Section;
use crate::theme;
use crate::ui::{ButtonStyle, Icon, button, dot, hint, icon, icon_button, tooltip};
use crate::worktrees::DialogMode;

/// Entre deux lectures : trois recherches GraphQL, quelques points sur 5 000 par heure.
const POLL_INTERVAL: Duration = Duration::from_secs(120);
/// À l'ouverture du panneau, des données plus vieilles que ça sont relues.
const STALE_AFTER: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct GitHub {
    /// `None` : pas encore cherché, ou aucun (ni saisi, ni `gh`).
    pub token: Option<Token>,
    /// Le jeton a été cherché au moins une fois.
    pub resolved: bool,
    pub inbox: Option<Inbox>,
    pub fetched_at: Option<SystemTime>,
    /// Échec de la dernière lecture.
    pub error: Option<String>,
    pub loading: bool,
    /// Relire dès la fin de la lecture en cours (jeton changé entre-temps).
    again: bool,
    /// Dépôts GitHub des remotes de chaque projet : `(remote, owner/repo)`.
    pub repos: HashMap<ProjectId, Vec<(String, String)>>,
    pub panel: Option<Panel>,
    /// PR dont on prépare l'espace de travail (fetch, worktree) : son URL.
    pub opening: Option<String>,
    /// Échec de la dernière action du panneau.
    pub action_error: Option<String>,
    refresh: Option<Task<()>>,
    open_task: Option<Task<()>>,
    _poll: Option<Task<()>>,
}

/// Panneau « Pull requests », affiché à la place des onglets.
pub(crate) struct Panel {
    pub focus: FocusHandle,
    /// Toutes les PR, sinon celles du projet actif.
    pub all: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrAction {
    /// Reviewer une PR (ou mettre la review à jour).
    Review,
    /// Traiter les commentaires reçus sur une de mes PR.
    Address,
}

/// Résultat d'une lecture en tâche de fond.
struct Fetched {
    repos: HashMap<ProjectId, Vec<(String, String)>>,
    token: Option<Token>,
    result: Option<Result<Inbox, FetchError>>,
}

impl GroveView {
    /// Lit les PR maintenant, puis toutes les deux minutes.
    pub(crate) fn start_github(&mut self, cx: &mut Context<Self>) {
        self.github._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh_github(cx)).is_err() {
                    break;
                }
                gpui::Timer::after(POLL_INTERVAL).await;
            }
        }));
    }

    /// Relit les remotes des projets, le jeton si besoin, puis les PR.
    pub(crate) fn refresh_github(&mut self, cx: &mut Context<Self>) {
        if self.github.loading {
            self.github.again = true;
            return;
        }
        self.github.loading = true;
        let roots: Vec<(ProjectId, PathBuf)> = self.state.projects.iter().map(|p| (p.id, p.root.clone())).collect();
        let (token_file, shell, token) = (self.paths.github_token_file(), self.config.shell(), self.github.token.clone());
        let work = cx.background_executor().spawn(async move {
            let repos = roots.into_iter().map(|(id, root)| (id, project_repos(&root))).collect();
            let mut token = token.or_else(|| grove_github::resolve_token(&token_file, &shell));
            let mut result = token.as_ref().map(|t| grove_github::fetch_inbox(&t.value));
            // Jeton de `gh` renouvelé, ou saisi entre-temps : on le relit une fois.
            if matches!(result, Some(Err(FetchError::Unauthorized))) {
                let fresh = grove_github::resolve_token(&token_file, &shell);
                if fresh != token {
                    token = fresh;
                    result = token.as_ref().map(|t| grove_github::fetch_inbox(&t.value));
                }
            }
            Fetched { repos, token, result }
        });
        self.github.refresh = Some(cx.spawn(async move |this, cx| {
            let fetched = work.await;
            let _ = this.update(cx, |this, cx| this.apply_fetched(fetched, cx));
        }));
        cx.notify();
    }

    fn apply_fetched(&mut self, fetched: Fetched, cx: &mut Context<Self>) {
        let previous = self.github.inbox.take();
        let github = &mut self.github;
        github.loading = false;
        github.resolved = true;
        github.repos = fetched.repos;
        github.token = fetched.token;
        match fetched.result {
            None => github.error = None,
            Some(Ok(inbox)) => {
                github.inbox = Some(inbox);
                github.error = None;
                github.fetched_at = Some(SystemTime::now());
            }
            Some(Err(FetchError::Unauthorized)) => {
                github.error = Some(match github.token.as_ref().map(|t| t.source) {
                    Some(TokenSource::Manual) => "Jeton GitHub refusé : il a expiré ou n'a pas les droits. Remplace-le dans les Paramètres.".into(),
                    _ => "Jeton de la CLI GitHub refusé : relance « gh auth login ».".into(),
                });
            }
            // Coupure réseau : on garde les dernières données.
            Some(Err(err)) => {
                github.inbox = previous;
                github.error = Some(format!("Lecture des PR impossible : {err}"));
            }
        }
        if std::mem::take(&mut self.github.again) {
            self.github.token = None;
            self.refresh_github(cx);
        }
        cx.notify();
    }

    /// Projet dont un remote pointe sur `repo` (`owner/repo`), et ce remote.
    fn project_for_repo(&self, repo: &str) -> Option<(ProjectId, String)> {
        self.state.projects.iter().find_map(|p| {
            let (remote, _) = self.github.repos.get(&p.id)?.iter().find(|(_, r)| r.eq_ignore_ascii_case(repo))?;
            Some((p.id, remote.clone()))
        })
    }

    /// PR ouverte de la branche `branch` du projet, parmi celles qui me concernent.
    pub(crate) fn workspace_pull(&self, project: ProjectId, branch: &str) -> Option<&PullRequest> {
        let repos = self.github.repos.get(&project)?;
        self.github.inbox.as_ref()?.all().find(|pr| {
            (pr.head_ref == branch || pr.local_branch() == branch) && repos.iter().any(|(_, r)| r.eq_ignore_ascii_case(&pr.repo))
        })
    }

    /// Espace de travail du projet extrait sur `branch`.
    fn workspace_on_branch(&self, project: ProjectId, branch: &str) -> Option<WorkspaceKey> {
        let p = self.state.project(project)?;
        p.workspaces
            .iter()
            .find(|w| self.git.get(&w.id).and_then(|g| g.head.as_ref()).is_some_and(|h| matches!(h, grove_git::Head::Branch(b) if b == branch)))
            .map(|w| (project, w.id))
    }

    /// Jeton saisi dans les Paramètres ; vide : on revient à celui de `gh`.
    pub(crate) fn set_github_token(&mut self, token: Option<&str>, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let path = self.paths.github_token_file();
        match token {
            Some(token) => grove_github::save_manual_token(&path, token)?,
            None => grove_github::remove_manual_token(&path)?,
        }
        self.github.token = None;
        self.github.inbox = None;
        self.github.error = None;
        self.refresh_github(cx);
        Ok(())
    }

    pub(crate) fn toggle_pull_requests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.github.panel.take().is_some() {
            self.focus_active(window, cx);
            return cx.notify();
        }
        self.leave_settings(cx);
        self.claude.history = None;
        let focus = cx.focus_handle();
        window.focus(&focus);
        // Le projet actif d'abord, s'il est sur GitHub.
        let all = self.state.active_workspace.is_none_or(|(p, _)| self.github.repos.get(&p).is_none_or(Vec::is_empty));
        self.github.panel = Some(Panel { focus, all });
        self.github.action_error = None;
        if self.github.fetched_at.is_none_or(|at| at.elapsed().unwrap_or_default() > STALE_AFTER) {
            self.refresh_github(cx);
        }
        cx.notify();
    }

    fn pull_requests_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.toggle_pull_requests(window, cx);
            cx.stop_propagation();
        }
    }

    /// Ouvre une session Claude sur la branche de la PR : dans l'espace qui
    /// l'a déjà extraite, sinon dans un nouveau worktree (specs §12.1).
    fn start_pull_session(&mut self, pr: PullRequest, action: PrAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.github.opening.is_some() {
            return;
        }
        self.github.action_error = None;
        let Some((project, remote)) = self.project_for_repo(&pr.repo) else {
            self.github.action_error =
                Some(format!("Aucun projet de Grove n'a de remote vers {} : ajoute son dossier (Ctrl+Shift+O).", pr.repo));
            return cx.notify();
        };
        let Some(root) = self.state.project(project).map(|p| p.root.clone()) else { return };
        let prompt = self.pull_prompt(&root, &pr, action);
        let branch = pr.local_branch();
        if let Some(key) = self.workspace_on_branch(project, &branch) {
            self.github.panel = None;
            return self.open_claude_with_prompt(key, prompt, window, cx);
        }

        self.github.opening = Some(pr.url.clone());
        let work = cx.background_executor().spawn(async move { prepare_pull_worktree(&root, &remote, &pr) });
        self.github.open_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.github.opening = None;
                match result {
                    Ok(created) => {
                        this.github.panel = None;
                        this.add_created_worktree(project, created, true, Some(prompt), window, cx);
                    }
                    Err(err) => this.github.action_error = Some(format!("Préparation de la branche « {branch} » : {err:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Nouvel onglet Claude dans `key`, qui reçoit `prompt` au démarrage.
    fn open_claude_with_prompt(&mut self, key: WorkspaceKey, prompt: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.config.claude.view;
        let Some(workspace) = self.state.workspace_mut(key) else { return };
        let tab = workspace.add_claude_tab(None, view);
        self.pending_prompts.insert(tab, prompt);
        self.activate_workspace(key, window, cx);
    }

    /// Skill ou prompt configuré (`.grove.toml` du projet d'abord), URL de la PR, contexte.
    fn pull_prompt(&self, root: &Path, pr: &PullRequest, action: PrAction) -> String {
        let project = ProjectConfig::load(root).unwrap_or_default().github;
        let github = &self.config.github;
        match action {
            PrAction::Review => {
                let instruction = project.review.prompt.unwrap_or_else(|| github.review.prompt.clone());
                let mut prompt = with_url(&instruction, &pr.url);
                if matches!(pr.review_state(), ReviewState::NewCommits | ReviewState::NewReplies) {
                    prompt.push_str(&format!("\n\n{}", github.review.refresh_prompt));
                }
                prompt
            }
            PrAction::Address => {
                let instruction = project.address.prompt.unwrap_or_else(|| github.address.prompt.clone());
                address_prompt(&instruction, pr)
            }
        }
    }

    // Rendu.

    pub(crate) fn render_pull_requests(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let panel = self.github.panel.as_ref()?;
        let github = &self.github;
        let active_project = self.state.active_workspace.map(|(p, _)| p);
        let project_repos: Vec<&str> =
            active_project.and_then(|p| github.repos.get(&p)).into_iter().flatten().map(|(_, r)| r.as_str()).collect();
        let visible = |pr: &&PullRequest| panel.all || project_repos.iter().any(|r| r.eq_ignore_ascii_case(&pr.repo));

        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .child(icon(Icon::GitPullRequest, theme::text_muted()))
            .child(div().text_color(theme::text()).font_weight(gpui::FontWeight::MEDIUM).child("Pull requests"))
            .when(!project_repos.is_empty(), |d| {
                let project = active_project.and_then(|p| self.state.project(p)).map(|p| p.name.clone()).unwrap_or_default();
                d.child(scope_toggle(&project, panel.all, cx))
            })
            .child(div().flex_1())
            .child(div().text_xs().text_color(theme::text_faint()).child(self.fetch_status()))
            .child(
                icon_button("pull-requests-refresh", Icon::Refresh)
                    .tooltip(tooltip("Actualiser"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_github(cx))),
            )
            .child(hint("Échap", "fermer").text_xs().text_color(theme::text_faint()));

        let mut body = div().id("pull-requests-list").flex_1().overflow_y_scroll().pb_4().flex().flex_col().gap_1();
        for error in [&github.error, &github.action_error].into_iter().flatten() {
            body = body.child(banner(error.clone(), theme::danger()));
        }
        body = match (&github.inbox, &github.token) {
            (Some(inbox), _) => {
                let to_review: Vec<_> = inbox.to_review.iter().filter(visible).collect();
                let authored: Vec<_> = inbox.authored.iter().filter(visible).collect();
                body.child(section_header("À reviewer", to_review.len()))
                    .children(empty_line(to_review.is_empty(), "Aucune PR à reviewer."))
                    .children(to_review.into_iter().map(|pr| self.pull_row(pr, PrAction::Review, cx)))
                    .child(section_header("Mes pull requests", authored.len()))
                    .children(empty_line(authored.is_empty(), "Aucune PR ouverte."))
                    .children(authored.into_iter().map(|pr| self.pull_row(pr, PrAction::Address, cx)))
            }
            (None, _) if github.loading || !github.resolved => {
                body.child(div().px_3().py_2().text_color(theme::text_muted()).child("Lecture des pull requests…"))
            }
            (None, None) => body.child(no_token(cx)),
            (None, Some(_)) => body,
        };

        Some(
            div()
                .id("pull-requests-view")
                .track_focus(&panel.focus)
                .on_key_down(cx.listener(Self::pull_requests_key_down))
                .size_full()
                .flex()
                .justify_center()
                .text_sm()
                .child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .h_full()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .px_6()
                        .pt_6()
                        .child(header)
                        .child(div().h(px(1.0)).mx_3().bg(theme::border()))
                        .child(body),
                )
                .into_any_element(),
        )
    }

    /// « @login · il y a 2 min », ou ce qui se passe.
    fn fetch_status(&self) -> String {
        let github = &self.github;
        if github.loading {
            return "actualisation…".into();
        }
        let source = match github.token.as_ref().map(|t| t.source) {
            Some(TokenSource::GhCli) => " (gh)",
            Some(TokenSource::Manual) => " (jeton saisi)",
            None => "",
        };
        match (&github.inbox, github.fetched_at) {
            (Some(inbox), Some(at)) => format!("@{}{source} · {}", inbox.viewer, crate::claude::relative_time(at)),
            _ => String::new(),
        }
    }

    fn pull_row(&self, pr: &PullRequest, action: PrAction, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = SharedString::from(format!("pull-{}", pr.url));
        let updated = UNIX_EPOCH + Duration::from_secs(pr.updated_at.max(0) as u64);
        let subtitle = format!(
            "{} #{} · @{} · {} · {}",
            pr.repo,
            pr.number,
            pr.author,
            pr.head_ref,
            crate::claude::relative_time(updated)
        );
        let opening = self.github.opening.as_deref() == Some(pr.url.as_str());
        let (badge, badge_color) = badge(pr, action);
        let (label, style) = match action {
            PrAction::Review => match pr.review_state() {
                ReviewState::Requested => (Some("Reviewer"), ButtonStyle::Primary),
                ReviewState::NewCommits | ReviewState::NewReplies => (Some("Actualiser la review"), ButtonStyle::Primary),
                ReviewState::ChangesRequested | ReviewState::Reviewed => (Some("Reviewer"), ButtonStyle::Secondary),
            },
            // Rien à faire tant que le reviewer n'est pas revenu.
            PrAction::Address => match pr.author_state() {
                AuthorState::ToAddress(_) | AuthorState::ChangesRequested => {
                    (Some("Prendre en compte les commentaires"), ButtonStyle::Primary)
                }
                _ => (None, ButtonStyle::Secondary),
            },
        };
        let url = pr.url.clone();
        div()
            .id(id.clone())
            .group(id.clone())
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_lg()
            .hover(|s| s.bg(theme::hover()))
            .cursor_pointer()
            .tooltip(tooltip("Ouvrir sur GitHub"))
            .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
            .child(icon(Icon::GitPullRequest, if pr.draft { theme::text_faint() } else { theme::accent() }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().flex().text_color(theme::text()).child(marquee(SharedString::from(format!("pull-title-{}", pr.url)), pr.title.clone())))
                    .child(
                        div()
                            .flex()
                            .text_xs()
                            .text_color(theme::text_faint())
                            .child(marquee(SharedString::from(format!("pull-meta-{}", pr.url)), subtitle)),
                    ),
            )
            .children(badge.map(|text| div().flex_none().text_xs().text_color(badge_color).child(text)))
            .children(pr.checks.map(|state| {
                let (color, text) = checks_look(state);
                div()
                    .id(SharedString::from(format!("pull-checks-{}", pr.url)))
                    .flex_none()
                    .p_1()
                    .tooltip(tooltip(format!("CI : {text}")))
                    .child(dot(color))
            }))
            .children(label.map(|label| {
                let pr = pr.clone();
                button(SharedString::from(format!("pull-action-{}", pr.url)), if opening { "Préparation…" } else { label }, style)
                    .when(opening, |b| b.opacity(0.6))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.start_pull_session(pr.clone(), action, window, cx);
                    }))
            }))
            .into_any_element()
    }
}

/// Dépôts GitHub des remotes de `root` : `(remote, owner/repo)`, `origin` d'abord.
fn project_repos(root: &Path) -> Vec<(String, String)> {
    let mut repos: Vec<_> = grove_git::remotes(root)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, url)| Some((name, grove_github::repo_from_remote(&url)?)))
        .collect();
    repos.sort_by_key(|(name, _)| name != "origin");
    repos
}

/// Extrait la branche de la PR (hors du thread de l'UI) puis crée son worktree.
/// Une PR venue d'un fork n'a pas sa branche sur le remote : on prend `refs/pull/<n>/head`.
fn prepare_pull_worktree(root: &Path, remote: &str, pr: &PullRequest) -> anyhow::Result<crate::worktrees::Created> {
    let branch = pr.local_branch();
    let target = if pr.cross_repository {
        grove_git::fetch(root, remote, &format!("+refs/pull/{}/head:refs/heads/{branch}", pr.number))?;
        NewWorktree::Existing(branch)
    } else {
        grove_git::fetch(root, remote, &format!("+refs/heads/{0}:refs/remotes/{remote}/{0}", pr.head_ref))?;
        let local = grove_git::list_branches(root)?.into_iter().any(|b| !b.remote && b.name == branch);
        if local { NewWorktree::Existing(branch) } else { NewWorktree::Remote(format!("{remote}/{}", pr.head_ref)) }
    };
    crate::worktrees::prepare_worktree(root, &target, DialogMode::Create)
}

/// Un skill (`/review`) reçoit l'URL en argument ; un prompt libre, à la ligne.
fn with_url(instruction: &str, url: &str) -> String {
    let instruction = instruction.trim();
    if instruction.starts_with('/') { format!("{instruction} {url}") } else { format!("{instruction}\n\nPR : {url}") }
}

/// Prompt pour traiter les commentaires : les fils qui attendent ma réponse en
/// contexte (ceux où j'ai déjà répondu ou poussé depuis sont laissés de côté).
fn address_prompt(instruction: &str, pr: &PullRequest) -> String {
    let mut prompt = with_url(instruction, &pr.url);
    let threads: Vec<&Thread> = pr.threads_to_address().collect();
    if threads.is_empty() {
        prompt.push_str("\n\nPrends en compte les changements demandés dans les reviews de cette PR.");
        return prompt;
    }
    prompt.push_str("\n\nPrends en compte ces commentaires de review non résolus :\n");
    for thread in threads {
        let place = match (&thread.path, thread.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.clone(),
            (None, _) => "commentaire général".to_owned(),
        };
        prompt.push_str(&format!("\n- {place} (@{}) : {}", thread.author, thread.body.replace('\n', "\n  ")));
    }
    prompt
}

/// État de la PR, du point de vue du reviewer ou de l'auteur.
fn badge(pr: &PullRequest, action: PrAction) -> (Option<String>, Hsla) {
    if action == PrAction::Review {
        return match pr.review_state() {
            ReviewState::Requested => (Some("à reviewer".into()), theme::waiting()),
            ReviewState::NewCommits => (Some("nouveaux commits".into()), theme::accent()),
            ReviewState::NewReplies => (Some("nouvelles réponses".into()), theme::accent()),
            ReviewState::ChangesRequested => (Some("changements demandés, en attente de l'auteur".into()), theme::text_faint()),
            ReviewState::Reviewed => (Some("review envoyée".into()), theme::text_faint()),
        };
    }
    match pr.author_state() {
        AuthorState::ToAddress(n) => (Some(format!("{n} commentaire{} à traiter", if n > 1 { "s" } else { "" })), theme::waiting()),
        AuthorState::ChangesRequested => (Some("changements demandés".into()), theme::danger()),
        AuthorState::WaitingForReviewer => (Some("traitée, en attente du reviewer".into()), theme::text_faint()),
        AuthorState::Approved => (Some("approuvée".into()), theme::accent()),
        AuthorState::Draft => (Some("brouillon".into()), theme::text_faint()),
        AuthorState::Open => (None, theme::text_faint()),
    }
}

pub(crate) fn checks_look(state: CheckState) -> (Hsla, &'static str) {
    match state {
        CheckState::Success => (theme::accent(), "réussie"),
        CheckState::Failure => (theme::danger(), "en échec"),
        CheckState::Pending => (theme::waiting(), "en cours"),
    }
}

fn scope_toggle(project: &str, all: bool, cx: &mut Context<GroveView>) -> impl IntoElement {
    let option = |id: &'static str, label: SharedString, selected: bool, value: bool| {
        div()
            .id(id)
            .h(px(22.0))
            .px_2()
            .flex()
            .items_center()
            .rounded_sm()
            .text_xs()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::selected()).text_color(theme::text()))
            .when(!selected, |d| d.text_color(theme::text_muted()).hover(|s| s.text_color(theme::text())))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(panel) = &mut this.github.panel {
                    panel.all = value;
                }
                cx.notify();
            }))
            .child(label)
    };
    div()
        .flex()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded_md()
        .border_1()
        .border_color(theme::border_strong())
        .child(option("pull-scope-project", project.to_owned().into(), !all, false))
        .child(option("pull-scope-all", "Tous les projets".into(), all, true))
}

fn section_header(title: &'static str, count: usize) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme::text_muted())
        .child(title)
        .child(div().text_color(theme::text_faint()).child(count.to_string()))
}

fn empty_line(empty: bool, text: &'static str) -> Option<impl IntoElement> {
    empty.then(|| div().px_3().py_1().text_color(theme::text_faint()).child(text))
}

fn banner(text: String, color: Hsla) -> impl IntoElement {
    div()
        .mx_3()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(color.opacity(0.4))
        .bg(color.opacity(0.08))
        .text_color(theme::text())
        .child(text)
}

fn no_token(cx: &mut Context<GroveView>) -> impl IntoElement {
    div()
        .mx_3()
        .mt_2()
        .p_4()
        .flex()
        .flex_col()
        .gap_2()
        .rounded_lg()
        .border_1()
        .border_color(theme::border_strong())
        .bg(theme::panel())
        .child(div().text_color(theme::text()).font_weight(gpui::FontWeight::MEDIUM).child("Pas de jeton GitHub"))
        .child(div().text_color(theme::text_muted()).child(
            "Connecte la CLI GitHub (« gh auth login » dans un terminal), ou saisis un jeton d'accès dans les Paramètres.",
        ))
        .child(div().flex().child(button("pull-requests-settings", "Paramètres GitHub", ButtonStyle::Secondary).on_click(
            cx.listener(|this, _, window, cx| {
                this.github.panel = None;
                this.open_settings(Some(Section::GitHub), None, window, cx);
            }),
        )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PullRequest {
        PullRequest {
            repo: "o/app".into(),
            number: 12,
            title: "Titre".into(),
            url: "https://github.com/o/app/pull/12".into(),
            author: "moi".into(),
            head_ref: "feat-1".into(),
            cross_repository: false,
            draft: false,
            updated_at: 0,
            review_decision: None,
            checks: None,
            last_commit_at: None,
            requested: false,
            my_review_at: None,
            my_review_changes: false,
            changes_requested_at: None,
            threads: Vec::new(),
            review_comments: Vec::new(),
        }
    }

    fn thread(path: &str, line: Option<u32>, body: &str, mine_last: bool) -> Thread {
        Thread { path: Some(path.into()), line, author: "rev".into(), body: body.into(), mine_last, last_at: Some(100) }
    }

    #[test]
    fn address_prompt_lists_threads() {
        let mut pr = pr();
        assert!(address_prompt("/fix", &pr).starts_with("/fix https://github.com/o/app/pull/12\n\nPrends en compte les changements"));
        pr.threads = vec![
            thread("src/a.rs", Some(4), "Nom ?\nÀ revoir", false),
            thread("c.rs", None, "Déjà répondu", true),
            thread("b.md", None, "Typo", false),
        ];
        pr.review_comments = vec![Thread { path: None, ..thread("", None, "Globalement OK", false) }];
        let prompt = address_prompt("/fix", &pr);
        assert!(
            prompt.ends_with("\n- commentaire général (@rev) : Globalement OK\n- src/a.rs:4 (@rev) : Nom ?\n  À revoir\n- b.md (@rev) : Typo"),
            "{prompt}"
        );
    }

    #[test]
    fn free_prompts_get_the_url_on_their_own_line() {
        let mut pr = pr();
        pr.threads = vec![thread("a.rs", Some(1), "Typo", false)];
        assert_eq!(
            address_prompt("Analyse les commentaires.", &pr),
            "Analyse les commentaires.\n\nPR : https://github.com/o/app/pull/12\n\n\
             Prends en compte ces commentaires de review non résolus :\n\n- a.rs:1 (@rev) : Typo"
        );
        assert_eq!(with_url(" /morty:review ", &pr.url), "/morty:review https://github.com/o/app/pull/12");
    }

    #[test]
    fn badges_follow_review_state() {
        let mut pr = pr();
        assert_eq!(badge(&pr, PrAction::Address).0, None);
        pr.draft = true;
        assert_eq!(badge(&pr, PrAction::Address).0.as_deref(), Some("brouillon"));
        pr.threads.push(thread("a", None, "x", true));
        assert_eq!(badge(&pr, PrAction::Address).0.as_deref(), Some("traitée, en attente du reviewer"));
        pr.threads.push(thread("b", None, "y", false));
        assert_eq!(badge(&pr, PrAction::Address).0.as_deref(), Some("1 commentaire à traiter"));
        // Poussé après le commentaire : traité aussi.
        pr.last_commit_at = Some(200);
        assert_eq!(badge(&pr, PrAction::Address).0.as_deref(), Some("traitée, en attente du reviewer"));
        pr.requested = true;
        assert_eq!(badge(&pr, PrAction::Review).0.as_deref(), Some("à reviewer"));
    }
}
