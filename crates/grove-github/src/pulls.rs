//! Pull requests ouvertes qui me concernent (specs §12.1) : celles où je suis
//! reviewer (review demandée ou déjà donnée) et celles dont je suis l'auteur.
//! Une seule requête GraphQL, avec trois recherches.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

const URL: &str = "https://api.github.com/graphql";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Au-delà, GitHub pagine ; personne n'a autant de PR ouvertes à suivre.
const PAGE: u32 = 50;
/// Corps d'un commentaire gardé pour le contexte de Claude.
const MAX_COMMENT: usize = 1500;

const QUERY: &str = r#"
query {
  viewer { login }
  requested: search(query: "is:open is:pr archived:false review-requested:@me", type: ISSUE, first: PAGE) { nodes { ...pr } }
  reviewed: search(query: "is:open is:pr archived:false reviewed-by:@me -author:@me", type: ISSUE, first: PAGE) { nodes { ...pr } }
  authored: search(query: "is:open is:pr archived:false author:@me", type: ISSUE, first: PAGE) { nodes { ...pr } }
}
fragment pr on PullRequest {
  number title url isDraft updatedAt
  repository { nameWithOwner }
  author { login }
  headRefName isCrossRepository
  reviewDecision
  latestReviews(first: 30) { nodes { author { login } state submittedAt body } }
    comments(last: 30) { nodes { author { login } createdAt } }
  commits(last: 1) { nodes { commit { committedDate statusCheckRollup { state } } } }
  reviewThreads(first: 100) {
    nodes {
      isResolved path line
      first: comments(first: 1) { nodes { author { login } body } }
      last: comments(last: 1) { nodes { author { login } createdAt } }
    }
  }
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Success,
    Failure,
    Pending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// Où j'en suis de la review d'une PR dont je suis reviewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewState {
    /// Review demandée (à moi ou à une de mes équipes).
    Requested,
    /// Des commits ont été poussés depuis ma dernière review.
    NewCommits,
    /// On m'a répondu dans un fil depuis ma dernière review.
    NewReplies,
    /// J'ai demandé des changements ; rien de neuf depuis.
    ChangesRequested,
    /// Rien de neuf depuis ma review.
    Reviewed,
}

/// Où en est une de mes PR, côté commentaires reçus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorState {
    /// Fils où le reviewer a parlé en dernier, sans commit poussé depuis.
    ToAddress(usize),
    /// Changements demandés, sans commit poussé depuis.
    ChangesRequested,
    /// J'ai répondu ou poussé : au reviewer de revenir.
    WaitingForReviewer,
    Approved,
    Draft,
    Open,
}

/// Fil de review non résolu, ou commentaire général d'une review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    /// `None` : commentaire général d'une review, sur aucun fichier.
    pub path: Option<String>,
    pub line: Option<u32>,
    /// Auteur et texte du premier commentaire.
    pub author: String,
    pub body: String,
    /// Le dernier message du fil est le mien.
    pub mine_last: bool,
    /// Date du dernier message.
    pub last_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub head_ref: String,
    /// La branche vient d'un fork : elle n'est pas sur le remote du dépôt.
    pub cross_repository: bool,
    pub draft: bool,
    /// Secondes depuis l'epoch.
    pub updated_at: i64,
    pub review_decision: Option<ReviewDecision>,
    /// Checks du dernier commit ; `None` : aucun.
    pub checks: Option<CheckState>,
    pub last_commit_at: Option<i64>,
    /// Trouvée par `review-requested:@me`.
    pub requested: bool,
    /// Date de ma dernière review.
    pub my_review_at: Option<i64>,
    /// Ma dernière review demandait des changements.
    pub my_review_changes: bool,
    /// Dernière demande de changements encore en vigueur (un reviewer qui a
    /// approuvé depuis ne compte plus).
    pub changes_requested_at: Option<i64>,
    /// Fils non résolus.
    pub threads: Vec<Thread>,
    /// Commentaires généraux de la dernière review de chaque reviewer (hors
    /// approbations) ; j'y ai répondu si j'ai commenté la PR depuis.
    pub review_comments: Vec<Thread>,
}

impl PullRequest {
    fn commit_after(&self, at: Option<i64>) -> bool {
        matches!((self.last_commit_at, at), (Some(commit), Some(at)) if commit > at)
    }

    pub fn review_state(&self) -> ReviewState {
        if self.requested {
            return ReviewState::Requested;
        }
        let Some(review) = self.my_review_at else { return ReviewState::Reviewed };
        let replied = self.threads.iter().any(|t| !t.mine_last && t.last_at.is_some_and(|at| at > review));
        if self.commit_after(Some(review)) {
            ReviewState::NewCommits
        } else if replied {
            ReviewState::NewReplies
        } else if self.my_review_changes {
            ReviewState::ChangesRequested
        } else {
            ReviewState::Reviewed
        }
    }

    /// Fils qui attendent ma réponse : le reviewer a parlé en dernier et je
    /// n'ai rien poussé depuis.
    pub fn threads_to_address(&self) -> impl Iterator<Item = &Thread> {
        self.review_comments.iter().chain(&self.threads).filter(|t| !t.mine_last && !self.commit_after(t.last_at))
    }

    pub fn author_state(&self) -> AuthorState {
        let to_address = self.threads_to_address().count();
        let changes = self.review_decision == Some(ReviewDecision::ChangesRequested);
        if to_address > 0 {
            AuthorState::ToAddress(to_address)
        } else if changes && !self.commit_after(self.changes_requested_at) {
            AuthorState::ChangesRequested
        } else if changes || !self.threads.is_empty() || !self.review_comments.is_empty() {
            AuthorState::WaitingForReviewer
        } else if self.review_decision == Some(ReviewDecision::Approved) {
            AuthorState::Approved
        } else if self.draft {
            AuthorState::Draft
        } else {
            AuthorState::Open
        }
    }

    /// Branche locale où l'extraire : la sienne, ou `pr-<n>` pour un fork.
    pub fn local_branch(&self) -> String {
        if self.cross_repository { format!("pr-{}", self.number) } else { self.head_ref.clone() }
    }
}

/// Mes pull requests ouvertes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inbox {
    pub viewer: String,
    /// Où je suis reviewer, les reviews demandées d'abord.
    pub to_review: Vec<PullRequest>,
    pub authored: Vec<PullRequest>,
}

impl Inbox {
    pub fn all(&self) -> impl Iterator<Item = &PullRequest> {
        self.to_review.iter().chain(&self.authored)
    }
}

#[derive(Debug)]
pub enum FetchError {
    /// Jeton refusé (révoqué, expiré, droits insuffisants).
    Unauthorized,
    Other(anyhow::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Unauthorized => write!(f, "jeton GitHub refusé"),
            FetchError::Other(err) => write!(f, "{err:#}"),
        }
    }
}

pub fn fetch_inbox(token: &str) -> Result<Inbox, FetchError> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).http_status_as_error(false).build().into();
    let body = json!({ "query": QUERY.replace("PAGE", &PAGE.to_string()) }).to_string();
    let mut response = agent
        .post(URL)
        .header("Authorization", &format!("bearer {token}"))
        .header("User-Agent", "grove")
        .header("Content-Type", "application/json")
        .send(body.as_str())
        .map_err(|err| FetchError::Other(err.into()))?;
    let status = response.status().as_u16();
    if status == 401 {
        return Err(FetchError::Unauthorized);
    }
    let text = response.body_mut().read_to_string().map_err(|err| FetchError::Other(err.into()))?;
    if !(200..300).contains(&status) {
        let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| Some(v.get("message")?.as_str()?.to_owned()));
        return Err(FetchError::Other(anyhow!("HTTP {status}{}", message.map(|m| format!(" : {m}")).unwrap_or_default())));
    }
    let value: Value = serde_json::from_str(&text).map_err(|err| FetchError::Other(err.into()))?;
    parse_inbox(&value).map_err(FetchError::Other)
}

pub(crate) fn parse_inbox(v: &Value) -> Result<Inbox> {
    if let Some(message) = v.pointer("/errors/0/message").and_then(Value::as_str) {
        return Err(anyhow!("GitHub : {message}"));
    }
    let data = v.get("data").context("réponse sans données")?;
    let viewer = data.pointer("/viewer/login").and_then(Value::as_str).context("réponse sans utilisateur")?.to_owned();
    let nodes = |key: &str| data.pointer(&format!("/{key}/nodes")).and_then(Value::as_array).cloned().unwrap_or_default();
    let parse = |node: &Value, requested| parse_pull(node, &viewer, requested);

    let mut to_review: Vec<PullRequest> = nodes("requested").iter().filter_map(|n| parse(n, true)).collect();
    for pr in nodes("reviewed").iter().filter_map(|n| parse(n, false)) {
        if !to_review.iter().any(|p| p.url == pr.url) {
            to_review.push(pr);
        }
    }
    // Ma propre PR ne se reviewe pas (l'auteur peut se demander une review).
    to_review.retain(|p| p.author != viewer);
    let authored = nodes("authored").iter().filter_map(|n| parse(n, false)).collect();
    Ok(Inbox { viewer, to_review, authored })
}

fn parse_pull(node: &Value, viewer: &str, requested: bool) -> Option<PullRequest> {
    let str_at = |path: &str| node.pointer(path).and_then(Value::as_str);
    let commit = node.pointer("/commits/nodes/0/commit");
    // Dernière review de chaque reviewer : `(auteur, état, date, texte)`.
    let reviews: Vec<(&str, &str, Option<i64>, &str)> = node
        .pointer("/latestReviews/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            let text = |key: &str| r.pointer(key).and_then(Value::as_str).unwrap_or_default();
            (text("/author/login"), text("/state"), parse_time(text("/submittedAt")), text("/body"))
        })
        .collect();
    let mine = reviews.iter().filter(|r| r.0 == viewer).max_by_key(|r| r.2);
    let changes_requested_at = reviews.iter().filter(|r| r.1 == "CHANGES_REQUESTED").filter_map(|r| r.2).max();
    // Mon dernier message dans la conversation de la PR.
    let my_comment_at = node
        .pointer("/comments/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.pointer("/author/login").and_then(Value::as_str) == Some(viewer))
        .filter_map(|c| c.get("createdAt").and_then(Value::as_str).and_then(parse_time))
        .max();
    let review_comments = reviews
        .iter()
        .filter(|(author, state, _, body)| {
            *author != viewer && matches!(*state, "COMMENTED" | "CHANGES_REQUESTED") && !body.trim().is_empty()
        })
        .map(|(author, _, at, body)| Thread {
            path: None,
            line: None,
            author: (*author).to_owned(),
            body: truncate(body.trim(), MAX_COMMENT),
            mine_last: matches!((my_comment_at, at), (Some(mine), Some(at)) if mine > *at),
            last_at: *at,
        })
        .collect();
    let threads = node
        .pointer("/reviewThreads/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|t| t.get("isResolved").and_then(Value::as_bool) == Some(false))
        .map(|t| {
            let text = |path: &str| t.pointer(path).and_then(Value::as_str).unwrap_or_default();
            Thread {
                path: Some(text("/path").to_owned()),
                line: t.get("line").and_then(Value::as_u64).map(|l| l as u32),
                author: text("/first/nodes/0/author/login").to_owned(),
                body: truncate(text("/first/nodes/0/body"), MAX_COMMENT),
                mine_last: text("/last/nodes/0/author/login") == viewer,
                last_at: parse_time(text("/last/nodes/0/createdAt")),
            }
        })
        .collect();
    Some(PullRequest {
        repo: str_at("/repository/nameWithOwner")?.to_owned(),
        number: node.get("number")?.as_u64()?,
        title: str_at("/title")?.to_owned(),
        url: str_at("/url")?.to_owned(),
        // Compte supprimé : GitHub renvoie `null`.
        author: str_at("/author/login").unwrap_or("ghost").to_owned(),
        head_ref: str_at("/headRefName")?.to_owned(),
        cross_repository: node.get("isCrossRepository").and_then(Value::as_bool).unwrap_or(false),
        draft: node.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        updated_at: str_at("/updatedAt").and_then(parse_time).unwrap_or(0),
        review_decision: match str_at("/reviewDecision") {
            Some("APPROVED") => Some(ReviewDecision::Approved),
            Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
            Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
            _ => None,
        },
        checks: match commit.and_then(|c| c.pointer("/statusCheckRollup/state")).and_then(Value::as_str) {
            Some("SUCCESS") => Some(CheckState::Success),
            Some("FAILURE" | "ERROR") => Some(CheckState::Failure),
            Some(_) => Some(CheckState::Pending),
            None => None,
        },
        last_commit_at: commit.and_then(|c| c.get("committedDate")).and_then(Value::as_str).and_then(parse_time),
        requested,
        my_review_at: mine.and_then(|r| r.2),
        my_review_changes: mine.is_some_and(|r| r.1 == "CHANGES_REQUESTED"),
        changes_requested_at,
        threads,
        review_comments,
    })
}

fn parse_time(text: &str) -> Option<i64> {
    text.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Appel réel avec le jeton de `gh` :
    /// `cargo test -p grove-github live_fetch -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_fetch() {
        let token = crate::resolve_token(std::path::Path::new("/nonexistent"), "sh").expect("pas de jeton gh");
        let inbox = fetch_inbox(&token.value).unwrap_or_else(|err| panic!("{err}"));
        println!("@{}", inbox.viewer);
        for pr in inbox.all() {
            let state = if pr.author == inbox.viewer { format!("{:?}", pr.author_state()) } else { format!("{:?}", pr.review_state()) };
            println!("{} #{} {state} {:?} {} fils", pr.repo, pr.number, pr.checks, pr.threads.len());
        }
    }

    fn pull(repo: &str, number: u64, author: &str) -> Value {
        json!({
            "number": number, "title": "Titre", "url": format!("https://github.com/{repo}/pull/{number}"),
            "isDraft": false, "updatedAt": "2026-10-27T10:00:00Z",
            "repository": { "nameWithOwner": repo }, "author": { "login": author },
            "headRefName": "feat-1", "isCrossRepository": false, "reviewDecision": "REVIEW_REQUIRED",
            "latestReviews": { "nodes": [
                { "author": { "login": "moi" }, "state": "COMMENTED", "submittedAt": "2026-10-27T09:00:00Z" },
                { "author": { "login": "autre" }, "state": "APPROVED", "submittedAt": "2026-10-27T11:00:00Z" }
            ] },
            "commits": { "nodes": [ { "commit": { "committedDate": "2026-10-27T09:30:00Z", "statusCheckRollup": { "state": "ERROR" } } } ] }
        })
    }

    #[test]
    fn parses_and_merges_searches() {
        let mut mine = pull("o/app", 3, "moi");
        mine["reviewThreads"] = json!({ "nodes": [
            { "isResolved": true, "path": "a.rs", "line": 1, "first": { "nodes": [] }, "last": { "nodes": [] } },
            thread("b.rs", "rev", "2026-10-27T10:00:00Z")
        ] });
        let inbox = parse_inbox(&json!({ "data": {
            "viewer": { "login": "moi" },
            "requested": { "nodes": [ pull("o/app", 1, "autre"), {} ] },
            "reviewed": { "nodes": [ pull("o/app", 1, "autre"), pull("o/lib", 2, "autre") ] },
            "authored": { "nodes": [ mine ] }
        } }))
        .unwrap();
        assert_eq!(inbox.viewer, "moi");
        let numbers: Vec<_> = inbox.to_review.iter().map(|p| (p.number, p.review_state())).collect();
        // #2 : un commit (9 h 30) après ma review (9 h).
        assert_eq!(numbers, [(1, ReviewState::Requested), (2, ReviewState::NewCommits)]);
        assert_eq!(inbox.to_review[0].checks, Some(CheckState::Failure));

        let mine = &inbox.authored[0];
        let expected = Thread {
            path: Some("b.rs".into()),
            line: None,
            author: "rev".into(),
            body: "Pourquoi ?".into(),
            mine_last: false,
            last_at: Some(1_793_095_200),
        };
        assert_eq!(mine.threads, [expected]);
        // Commentaire de 10 h, dernier commit à 9 h 30 : à traiter.
        assert_eq!(mine.author_state(), AuthorState::ToAddress(1));
    }

    /// Fil ouvert par `rev`, dont `last` a écrit le dernier message à `at`.
    fn thread(path: &str, last: &str, at: &str) -> Value {
        json!({
            "isResolved": false, "path": path, "line": null,
            "first": { "nodes": [ { "author": { "login": "rev" }, "body": "Pourquoi ?" } ] },
            "last": { "nodes": [ { "author": { "login": last }, "createdAt": at } ] }
        })
    }

    fn mine_with(threads: Vec<Value>, reviews: Value, commit_at: &str) -> PullRequest {
        let mut node = pull("o/app", 3, "moi");
        node["reviewThreads"] = json!({ "nodes": threads });
        node["latestReviews"] = json!({ "nodes": reviews });
        node["commits"]["nodes"][0]["commit"]["committedDate"] = json!(commit_at);
        parse_pull(&node, "moi", false).unwrap()
    }

    #[test]
    fn answered_or_pushed_threads_are_no_longer_to_address() {
        // J'ai répondu en dernier.
        let pr = mine_with(vec![thread("a.rs", "moi", "2026-10-27T10:00:00Z")], json!([]), "2026-10-27T09:00:00Z");
        assert_eq!(pr.author_state(), AuthorState::WaitingForReviewer);
        // J'ai poussé après le commentaire, sans répondre.
        let pr = mine_with(vec![thread("a.rs", "rev", "2026-10-27T10:00:00Z")], json!([]), "2026-10-27T11:00:00Z");
        assert_eq!(pr.author_state(), AuthorState::WaitingForReviewer);
        assert_eq!(pr.threads_to_address().count(), 0);
        // Le reviewer a relancé après ma réponse.
        let pr = mine_with(
            vec![thread("a.rs", "rev", "2026-10-27T12:00:00Z"), thread("b.rs", "moi", "2026-10-27T12:00:00Z")],
            json!([]),
            "2026-10-27T11:00:00Z",
        );
        assert_eq!(pr.author_state(), AuthorState::ToAddress(1));
    }

    #[test]
    fn review_summaries_count_as_comments() {
        // Une review « Comment » sans fil en ligne (rgsystemes/mimicus#106).
        let reviews = json!([
            { "author": { "login": "rev" }, "state": "COMMENTED", "submittedAt": "2026-10-27T10:00:00Z", "body": " À revoir\n" },
            { "author": { "login": "vide" }, "state": "COMMENTED", "submittedAt": "2026-10-27T10:00:00Z", "body": "" },
            { "author": { "login": "ok" }, "state": "APPROVED", "submittedAt": "2026-10-27T10:00:00Z", "body": "LGTM" }
        ]);
        let pr = mine_with(vec![], reviews.clone(), "2026-10-27T09:00:00Z");
        assert_eq!(pr.author_state(), AuthorState::ToAddress(1));
        let comment = pr.threads_to_address().next().unwrap();
        assert_eq!((comment.path.as_deref(), comment.author.as_str(), comment.body.as_str()), (None, "rev", "À revoir"));
        // J'ai répondu dans la conversation, ou poussé depuis.
        let mut node = pull("o/app", 3, "moi");
        node["latestReviews"] = json!({ "nodes": reviews.clone() });
        node["comments"] = json!({ "nodes": [ { "author": { "login": "moi" }, "createdAt": "2026-10-27T11:00:00Z" } ] });
        node["commits"]["nodes"][0]["commit"]["committedDate"] = json!("2026-10-27T09:00:00Z");
        assert_eq!(parse_pull(&node, "moi", false).unwrap().author_state(), AuthorState::WaitingForReviewer);
        let pr = mine_with(vec![], reviews, "2026-10-27T11:00:00Z");
        assert_eq!(pr.author_state(), AuthorState::WaitingForReviewer);
    }

    #[test]
    fn requested_changes_wait_for_a_push() {
        let reviews = json!([{ "author": { "login": "rev" }, "state": "CHANGES_REQUESTED", "submittedAt": "2026-10-27T10:00:00Z" }]);
        let mut pr = mine_with(vec![], reviews.clone(), "2026-10-27T09:00:00Z");
        pr.review_decision = Some(ReviewDecision::ChangesRequested);
        assert_eq!(pr.author_state(), AuthorState::ChangesRequested);
        let mut pr = mine_with(vec![], reviews, "2026-10-27T11:00:00Z");
        pr.review_decision = Some(ReviewDecision::ChangesRequested);
        assert_eq!(pr.author_state(), AuthorState::WaitingForReviewer);
    }

    #[test]
    fn reviewer_sees_replies_and_own_change_requests() {
        let reviews = json!([{ "author": { "login": "moi" }, "state": "CHANGES_REQUESTED", "submittedAt": "2026-10-27T10:00:00Z" }]);
        let mut node = pull("o/app", 4, "autre");
        node["latestReviews"] = json!({ "nodes": reviews });
        node["reviewThreads"] = json!({ "nodes": [ thread("a.rs", "moi", "2026-10-27T10:00:00Z") ] });
        let pr = parse_pull(&node, "moi", false).unwrap();
        assert_eq!(pr.review_state(), ReviewState::ChangesRequested);

        node["reviewThreads"] = json!({ "nodes": [ thread("a.rs", "autre", "2026-10-27T10:30:00Z") ] });
        assert_eq!(parse_pull(&node, "moi", false).unwrap().review_state(), ReviewState::NewReplies);

        node["commits"]["nodes"][0]["commit"]["committedDate"] = json!("2026-10-27T11:00:00Z");
        assert_eq!(parse_pull(&node, "moi", false).unwrap().review_state(), ReviewState::NewCommits);
    }

    #[test]
    fn reports_graphql_errors() {
        let err = parse_inbox(&json!({ "errors": [ { "message": "Bad credentials" } ] })).unwrap_err();
        assert!(err.to_string().contains("Bad credentials"));
    }

    #[test]
    fn fork_branches_get_their_own_name() {
        let mut node = pull("o/app", 7, "x");
        node["isCrossRepository"] = json!(true);
        let pr = parse_pull(&node, "moi", false).unwrap();
        assert_eq!(pr.local_branch(), "pr-7");
        assert_eq!(truncate("héhé", 2), "hé…");
    }
}
