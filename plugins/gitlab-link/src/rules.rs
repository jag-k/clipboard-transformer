//! GitLab project and resource rule implementation.

use std::collections::BTreeMap;

use extism_pdk::{http, Error, HttpRequest};
use serde::{Deserialize, Serialize};

use super::pdk::http_host_allowed;
use super::pdk::types::{
    CompileRuleRequest, CompileRuleResponse, CompileRuleResult, TransformRequest, TransformResponse,
};
use super::settings::{
    CommentDisplay, DiscussionRuleSettings, Instance, ProjectRuleSettings, RepositoryLinkKind,
    RepositoryRuleSettings, RuleSettings,
};
use super::StoredState;

mod rule_types {
    include!(concat!(env!("OUT_DIR"), "/rule_types.rs"));
}

use rule_types::{
    COMMIT, ISSUE, JOB, MERGE_REQUEST, MILESTONE, PIPELINE, PROJECT, REPOSITORY, TAG,
};

#[derive(Serialize, Deserialize)]
struct CompiledRule {
    comment_display: CommentDisplay,
    hosts: Vec<String>,
    kind: CompiledRuleKind,
    online: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CompiledRuleKind {
    Project { aliases: BTreeMap<String, String> },
    MergeRequest,
    Issue,
    Milestone,
    Pipeline,
    Job,
    Commit,
    Tag,
    Repository { kinds: Vec<RepositoryLinkKind> },
}

pub(crate) fn compile_rule(request: CompileRuleRequest) -> Result<CompileRuleResponse, Error> {
    let (mut hosts, online, comment_display, kind) = match request.rule_type.as_str() {
        PROJECT => {
            let settings: ProjectRuleSettings =
                match serde_json::from_value(serde_json::Value::Object(request.settings)) {
                    Ok(settings) => settings,
                    Err(error) => {
                        return Ok(compile_error(format!("invalid rule settings: {error}")))
                    }
                };
            for (page, label) in &settings.aliases {
                if !valid_alias_path(page) || label.trim().is_empty() {
                    return Ok(compile_error(format!(
                        "invalid project page alias {page:?}: {label:?}"
                    )));
                }
            }
            (
                settings.hosts,
                settings.online,
                CommentDisplay::Hidden,
                CompiledRuleKind::Project {
                    aliases: settings.aliases,
                },
            )
        }
        REPOSITORY => {
            let settings: RepositoryRuleSettings =
                match serde_json::from_value(serde_json::Value::Object(request.settings)) {
                    Ok(settings) => settings,
                    Err(error) => {
                        return Ok(compile_error(format!("invalid rule settings: {error}")))
                    }
                };
            if settings.kinds.is_empty() {
                return Ok(compile_error("kinds must not be empty".to_string()));
            }
            (
                settings.hosts,
                settings.online,
                CommentDisplay::Hidden,
                CompiledRuleKind::Repository {
                    kinds: settings.kinds,
                },
            )
        }
        MERGE_REQUEST | ISSUE => {
            let kind = match request.rule_type.as_str() {
                MERGE_REQUEST => CompiledRuleKind::MergeRequest,
                ISSUE => CompiledRuleKind::Issue,
                _ => unreachable!("matched discussion rule type"),
            };
            let settings: DiscussionRuleSettings =
                match serde_json::from_value(serde_json::Value::Object(request.settings)) {
                    Ok(settings) => settings,
                    Err(error) => {
                        return Ok(compile_error(format!("invalid rule settings: {error}")))
                    }
                };
            (
                settings.hosts,
                settings.online,
                settings.comment_display,
                kind,
            )
        }
        MILESTONE | PIPELINE | JOB | COMMIT | TAG => {
            let kind = match request.rule_type.as_str() {
                MILESTONE => CompiledRuleKind::Milestone,
                PIPELINE => CompiledRuleKind::Pipeline,
                JOB => CompiledRuleKind::Job,
                COMMIT => CompiledRuleKind::Commit,
                TAG => CompiledRuleKind::Tag,
                _ => unreachable!("matched non-discussion resource rule type"),
            };
            let settings: RuleSettings =
                match serde_json::from_value(serde_json::Value::Object(request.settings)) {
                    Ok(settings) => settings,
                    Err(error) => {
                        return Ok(compile_error(format!("invalid rule settings: {error}")))
                    }
                };
            (
                settings.hosts,
                settings.online,
                CommentDisplay::Hidden,
                kind,
            )
        }
        _ => {
            return Ok(compile_error(format!(
                "unknown rule type {:?}",
                request.rule_type
            )))
        }
    };
    if hosts.is_empty() {
        hosts = StoredState::load()
            .instances
            .iter()
            .map(|instance| instance.host.clone())
            .collect();
    }
    if hosts.is_empty() {
        hosts.push("gitlab.com".to_string());
    }
    for host in &hosts {
        if host.trim().is_empty() || host.contains('/') {
            return Ok(compile_error(format!("invalid host {host:?} in hosts")));
        }
    }
    let rule = serde_json::to_value(CompiledRule {
        comment_display,
        hosts,
        kind,
        online,
    })?
    .as_object()
    .cloned()
    .expect("CompiledRule serializes as an object");
    Ok(CompileRuleResponse {
        result: CompileRuleResult::Ok,
        rule: Some(rule),
        message: None,
    })
}

fn valid_alias_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['?', '#'])
        && !path.contains(char::is_whitespace)
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".." | "-"))
}

pub(crate) fn transform(request: TransformRequest) -> Result<TransformResponse, Error> {
    let rule: CompiledRule = serde_json::from_value(serde_json::Value::Object(request.rule))?;
    let value = request.value.trim();
    let Some(link) = parse_gitlab_link(value, &rule.hosts) else {
        return Ok(no_match());
    };
    let page_alias = match (&rule.kind, &link.kind) {
        (CompiledRuleKind::Project { .. }, ParsedLinkKind::Project) => Some(None),
        (CompiledRuleKind::Project { aliases }, ParsedLinkKind::ProjectPage(page)) => {
            aliases.get(page).map(|alias| Some(alias.as_str()))
        }
        (CompiledRuleKind::MergeRequest, ParsedLinkKind::MergeRequest) => Some(None),
        (CompiledRuleKind::Issue, ParsedLinkKind::Issue) => Some(None),
        (CompiledRuleKind::Milestone, ParsedLinkKind::Milestone) => Some(None),
        (CompiledRuleKind::Pipeline, ParsedLinkKind::Pipeline) => Some(None),
        (CompiledRuleKind::Job, ParsedLinkKind::Job) => Some(None),
        (CompiledRuleKind::Commit, ParsedLinkKind::Commit) => Some(None),
        (CompiledRuleKind::Tag, ParsedLinkKind::Tag) => Some(None),
        (CompiledRuleKind::Repository { kinds }, ParsedLinkKind::Repository { kind, .. })
            if kinds.contains(kind) =>
        {
            Some(None)
        }
        _ => None,
    };
    let Some(page_alias) = page_alias else {
        return Ok(no_match());
    };
    let state = rule.online.then(StoredState::load);
    let instance = state
        .as_ref()
        .and_then(|state| state.queryable_instance(&link.host));
    let title = instance.and_then(|instance| fetch_title(instance, &link));
    let repository_location = instance
        .and_then(|instance| resolve_repository_location(instance, &link))
        .or_else(|| resolve_commit_repository_location(&link));
    let comment_author = if matches!(
        rule.comment_display,
        CommentDisplay::Author | CommentDisplay::AuthorAndId
    ) {
        instance.and_then(|instance| fetch_comment_author(instance, &link))
    } else {
        None
    };
    let comment = link.comment_label(rule.comment_display, comment_author.as_deref());
    let text = format!(
        "[{}]({value})",
        link.label(
            title.as_deref(),
            page_alias,
            comment.as_deref(),
            repository_location.as_deref(),
        )
    );
    Ok(TransformResponse {
        action: "replace".to_string(),
        text: Some(text),
        message: Some(format!(
            "GitLab {} link converted to Markdown",
            link.kind_name()
        )),
    })
}

fn no_match() -> TransformResponse {
    TransformResponse {
        action: "no-match".to_string(),
        text: None,
        message: None,
    }
}

fn compile_error(message: String) -> CompileRuleResponse {
    CompileRuleResponse {
        result: CompileRuleResult::Error,
        rule: None,
        message: Some(message),
    }
}

fn fetch_title(instance: &Instance, link: &GitlabLink) -> Option<String> {
    let api_base = instance
        .api_base
        .clone()
        .unwrap_or_else(|| format!("https://{}/api/v4", instance.host));
    let project = link.project.replace('/', "%2F");
    let (url, title_pointer) = match &link.kind {
        ParsedLinkKind::MergeRequest => (
            format!(
                "{}/projects/{project}/merge_requests/{}",
                api_base.trim_end_matches('/'),
                link.number?
            ),
            "/title",
        ),
        ParsedLinkKind::Issue => (
            format!(
                "{}/projects/{project}/issues/{}",
                api_base.trim_end_matches('/'),
                link.number?
            ),
            "/title",
        ),
        ParsedLinkKind::Milestone => (
            format!(
                "{}/projects/{project}/milestones?iids%5B%5D={}",
                api_base.trim_end_matches('/'),
                link.number?
            ),
            "/0/title",
        ),
        ParsedLinkKind::Pipeline => (
            format!(
                "{}/projects/{project}/pipelines/{}",
                api_base.trim_end_matches('/'),
                link.number?
            ),
            "/name",
        ),
        ParsedLinkKind::Job => (
            format!(
                "{}/projects/{project}/jobs/{}",
                api_base.trim_end_matches('/'),
                link.number?
            ),
            "/name",
        ),
        ParsedLinkKind::Commit => (
            format!(
                "{}/projects/{project}/repository/commits/{}",
                api_base.trim_end_matches('/'),
                link.reference.as_deref()?.replace('/', "%2F")
            ),
            "/title",
        ),
        ParsedLinkKind::Tag => (
            format!(
                "{}/projects/{project}/repository/tags/{}",
                api_base.trim_end_matches('/'),
                link.reference.as_deref()?.replace('/', "%2F")
            ),
            "/commit/title",
        ),
        ParsedLinkKind::Project
        | ParsedLinkKind::ProjectPage(_)
        | ParsedLinkKind::Repository { .. } => (
            format!("{}/projects/{project}", api_base.trim_end_matches('/')),
            "/name",
        ),
    };
    let body = fetch_json(instance, url)?;
    let title = body.pointer(title_pointer)?.as_str()?.trim().to_string();
    (!title.is_empty()).then_some(title)
}

fn fetch_comment_author(instance: &Instance, link: &GitlabLink) -> Option<String> {
    let resource = match &link.kind {
        ParsedLinkKind::MergeRequest => "merge_requests",
        ParsedLinkKind::Issue => "issues",
        _ => return None,
    };
    let api_base = instance
        .api_base
        .clone()
        .unwrap_or_else(|| format!("https://{}/api/v4", instance.host));
    let url = format!(
        "{}/projects/{}/{resource}/{}/notes/{}",
        api_base.trim_end_matches('/'),
        link.project.replace('/', "%2F"),
        link.number?,
        link.comment_id?
    );
    let body = fetch_json(instance, url)?;
    let username = body
        .pointer("/author/username")?
        .as_str()?
        .trim()
        .to_string();
    (!username.is_empty()).then_some(username)
}

fn resolve_repository_location(instance: &Instance, link: &GitlabLink) -> Option<String> {
    let ParsedLinkKind::Repository { kind, locator } = &link.kind else {
        return None;
    };
    let reference_kind = match link.ref_type.as_deref()? {
        "heads" => ("branches", "branch"),
        "tags" => ("tags", "tag"),
        _ => return None,
    };
    let api_base = instance
        .api_base
        .clone()
        .unwrap_or_else(|| format!("https://{}/api/v4", instance.host));
    let project = link.project.replace('/', "%2F");
    let segments: Vec<&str> = locator.split('/').collect();
    let maximum_boundary = if matches!(
        kind,
        RepositoryLinkKind::Blob | RepositoryLinkKind::Raw | RepositoryLinkKind::Blame
    ) {
        segments.len().checked_sub(1)?
    } else {
        segments.len()
    };
    for boundary in (1..=maximum_boundary).rev() {
        let reference = segments[..boundary].join("/");
        let url = format!(
            "{}/projects/{project}/repository/{}/{}",
            api_base.trim_end_matches('/'),
            reference_kind.0,
            reference.replace('/', "%2F")
        );
        if fetch_json(instance, url).is_some() {
            let path = (boundary < segments.len()).then(|| segments[boundary..].join("/"));
            return Some(resolved_repository_location(
                *kind,
                &reference,
                reference_kind.1,
                path.as_deref(),
                link.line_selection,
            ));
        }
    }
    None
}

fn resolve_commit_repository_location(link: &GitlabLink) -> Option<String> {
    let ParsedLinkKind::Repository { kind, locator } = &link.kind else {
        return None;
    };
    let (reference, path) = locator.split_once('/')?;
    if reference.len() < 7 || !reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(resolved_repository_location(
        *kind,
        reference,
        "commit",
        Some(path),
        link.line_selection,
    ))
}

fn fetch_json(instance: &Instance, url: String) -> Option<serde_json::Value> {
    let host = http_url_host(&url)?;
    if !http_host_allowed(host).ok()? {
        return None;
    }
    let mut request = HttpRequest::new(url).with_method("GET");
    if let Some(token) = &instance.token {
        request = request.with_header("PRIVATE-TOKEN", token);
    }
    let response = http::request::<()>(&request, None).ok()?;
    if response.status_code() != 200 {
        return None;
    }
    serde_json::from_slice(&response.body()).ok()
}

pub(super) fn instance_api_host(instance: &Instance) -> Option<String> {
    match &instance.api_base {
        Some(api_base) => http_url_host(api_base),
        None => Some(instance.host.clone()),
    }
}

fn http_url_host(url: &str) -> Option<String> {
    let authority = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?
        .split('/')
        .next()?;
    if authority.is_empty() || authority.contains(['@', '?', '#']) || authority.starts_with(':') {
        return None;
    }
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        return Some(authority[..=end].to_string());
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

struct GitlabLink {
    host: String,
    project: String,
    kind: ParsedLinkKind,
    number: Option<u64>,
    reference: Option<String>,
    comment_id: Option<u64>,
    line_selection: Option<LineSelection>,
    ref_type: Option<String>,
}

#[derive(Clone, Copy)]
enum LineSelection {
    Line(u64),
    Range { start: u64, end: u64 },
}

enum ParsedLinkKind {
    Project,
    ProjectPage(String),
    MergeRequest,
    Issue,
    Milestone,
    Pipeline,
    Job,
    Commit,
    Tag,
    Repository {
        kind: RepositoryLinkKind,
        locator: String,
    },
}

impl GitlabLink {
    fn label(
        &self,
        title: Option<&str>,
        page_alias: Option<&str>,
        comment: Option<&str>,
        resolved_repository_location: Option<&str>,
    ) -> String {
        let reference = match &self.kind {
            ParsedLinkKind::Project => self.project.clone(),
            ParsedLinkKind::ProjectPage(_) => {
                format!("{} ({})", self.project, page_alias.expect("page has alias"))
            }
            ParsedLinkKind::MergeRequest => {
                format!("{}!{}", self.project, self.number.expect("MR has a number"))
            }
            ParsedLinkKind::Issue => {
                format!(
                    "{}#{}",
                    self.project,
                    self.number.expect("issue has a number")
                )
            }
            ParsedLinkKind::Milestone => format!(
                "{}%{}",
                self.project,
                self.number.expect("milestone has a number")
            ),
            ParsedLinkKind::Pipeline => format!(
                "{} Pipeline #{}",
                self.project,
                self.number.expect("pipeline has a number")
            ),
            ParsedLinkKind::Job => format!(
                "{} Job #{}",
                self.project,
                self.number.expect("job has a number")
            ),
            ParsedLinkKind::Commit => format!(
                "{}@{}",
                self.project,
                short_commit_reference(self.reference.as_deref().expect("commit has a reference"))
            ),
            ParsedLinkKind::Tag => format!(
                "{}@{} (Tag)",
                self.project,
                self.reference.as_deref().expect("tag has a reference")
            ),
            ParsedLinkKind::Repository { kind, locator } => format!(
                "{} ({})",
                self.project,
                resolved_repository_location
                    .map(str::to_string)
                    .unwrap_or_else(|| repository_location(*kind, locator, self.line_selection))
            ),
        };
        match (title, comment) {
            (Some(title), comment) => {
                let mut context = match (&self.kind, page_alias) {
                    (_, Some(alias)) => format!("{}, {alias}", self.project),
                    (ParsedLinkKind::Pipeline, None) => self.numbered_context("Pipeline"),
                    (ParsedLinkKind::Job, None) => self.numbered_context("Job"),
                    (ParsedLinkKind::Tag, None) => format!(
                        "{}@{}, Tag",
                        self.project,
                        self.reference.as_deref().expect("tag has a reference")
                    ),
                    (ParsedLinkKind::Repository { kind, locator }, None) => format!(
                        "{}, {}",
                        self.project,
                        resolved_repository_location
                            .map(str::to_string)
                            .unwrap_or_else(|| repository_location(
                                *kind,
                                locator,
                                self.line_selection
                            ))
                    ),
                    (_, None) => reference,
                };
                if let Some(comment) = comment {
                    context.push_str(", ");
                    context.push_str(comment);
                }
                format!("{title} ({context})")
            }
            (None, Some(comment)) => format!("{reference} ({comment})"),
            (None, None) => reference,
        }
    }

    fn comment_label(&self, display: CommentDisplay, author: Option<&str>) -> Option<String> {
        let id = self.comment_id?;
        match display {
            CommentDisplay::Hidden => None,
            CommentDisplay::Marker => Some("comment".to_string()),
            CommentDisplay::Id => Some(format!("comment {id}")),
            CommentDisplay::Author => Some(match author {
                Some(author) => format!("comment by @{author}"),
                None => format!("comment {id}"),
            }),
            CommentDisplay::AuthorAndId => Some(match author {
                Some(author) => format!("comment {id} by @{author}"),
                None => format!("comment {id}"),
            }),
        }
    }

    fn kind_name(&self) -> &'static str {
        match &self.kind {
            ParsedLinkKind::Project => "project",
            ParsedLinkKind::ProjectPage(_) => "project page",
            ParsedLinkKind::MergeRequest => "merge request",
            ParsedLinkKind::Issue => "issue",
            ParsedLinkKind::Milestone => "milestone",
            ParsedLinkKind::Pipeline => "pipeline",
            ParsedLinkKind::Job => "job",
            ParsedLinkKind::Commit => "commit",
            ParsedLinkKind::Tag => "tag",
            ParsedLinkKind::Repository { kind, .. } => repository_kind_name(*kind),
        }
    }

    fn numbered_context(&self, name: &str) -> String {
        format!(
            "{}, {name} #{}",
            self.project,
            self.number.expect("numbered resource has a number")
        )
    }
}

fn short_commit_reference(reference: &str) -> &str {
    if reference.len() > 8 && reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        &reference[..8]
    } else {
        reference
    }
}

fn repository_kind_label(kind: RepositoryLinkKind) -> &'static str {
    match kind {
        RepositoryLinkKind::Tree => "Tree",
        RepositoryLinkKind::Blob => "File",
        RepositoryLinkKind::Raw => "Raw file",
        RepositoryLinkKind::Blame => "Blame",
        RepositoryLinkKind::Commits => "Commits",
        RepositoryLinkKind::Compare => "Compare",
    }
}

fn repository_location(
    kind: RepositoryLinkKind,
    locator: &str,
    line_selection: Option<LineSelection>,
) -> String {
    let mut location = format!("{}: {locator}", repository_kind_label(kind));
    if matches!(
        kind,
        RepositoryLinkKind::Blob | RepositoryLinkKind::Raw | RepositoryLinkKind::Blame
    ) {
        match line_selection {
            Some(LineSelection::Line(line)) => location.push_str(&format!(", line {line}")),
            Some(LineSelection::Range { start, end }) => {
                location.push_str(&format!(", lines {start}–{end}"));
            }
            None => {}
        }
    }
    location
}

fn resolved_repository_location(
    kind: RepositoryLinkKind,
    reference: &str,
    reference_kind: &str,
    path: Option<&str>,
    line_selection: Option<LineSelection>,
) -> String {
    let locator = path.unwrap_or(reference);
    let mut location = repository_location(kind, locator, line_selection);
    if path.is_some() {
        location.push_str(&format!(", {reference_kind} {reference}"));
    }
    location
}

fn repository_kind_name(kind: RepositoryLinkKind) -> &'static str {
    match kind {
        RepositoryLinkKind::Tree => "repository tree",
        RepositoryLinkKind::Blob => "repository file",
        RepositoryLinkKind::Raw => "raw repository file",
        RepositoryLinkKind::Blame => "repository blame",
        RepositoryLinkKind::Commits => "commit list",
        RepositoryLinkKind::Compare => "repository comparison",
    }
}

fn parse_gitlab_link(value: &str, hosts: &[String]) -> Option<GitlabLink> {
    if value.contains(char::is_whitespace) {
        return None;
    }
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    if !hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return None;
    }
    let (path, fragment) = path
        .split_once('#')
        .map_or((path, None), |(path, fragment)| (path, Some(fragment)));
    let (path, query) = path
        .split_once('?')
        .map_or((path, None), |(path, query)| (path, Some(query)));
    let ref_type = query.and_then(|query| {
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == "ref_type").then(|| value.to_string())
        })
    });
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let marker = segments.iter().position(|segment| *segment == "-");
    let (project, kind, number, reference) = match marker {
        None if segments.len() >= 2 => (segments.join("/"), ParsedLinkKind::Project, None, None),
        Some(marker) if marker > 0 && segments.len() > marker + 1 => {
            let resource = &segments[marker + 1..];
            let numbered_kind = resource
                .get(1)
                .and_then(|number| number.parse::<u64>().ok())
                .and_then(|number| {
                    let kind = match resource[0] {
                        "merge_requests" => ParsedLinkKind::MergeRequest,
                        "issues" => ParsedLinkKind::Issue,
                        "milestones" => ParsedLinkKind::Milestone,
                        "pipelines" => ParsedLinkKind::Pipeline,
                        "jobs" => ParsedLinkKind::Job,
                        _ => return None,
                    };
                    Some((kind, number))
                });
            match numbered_kind {
                Some((kind, number)) => (segments[..marker].join("/"), kind, Some(number), None),
                None if resource.len() >= 2 && resource[0] == "commit" => (
                    segments[..marker].join("/"),
                    ParsedLinkKind::Commit,
                    None,
                    Some(resource[1].to_string()),
                ),
                None if resource.len() >= 2 && resource[0] == "tags" => (
                    segments[..marker].join("/"),
                    ParsedLinkKind::Tag,
                    None,
                    Some(resource[1..].join("/")),
                ),
                None if resource.len() >= 2 => {
                    let repository_kind = match resource[0] {
                        "tree" => Some(RepositoryLinkKind::Tree),
                        "blob" => Some(RepositoryLinkKind::Blob),
                        "raw" => Some(RepositoryLinkKind::Raw),
                        "blame" => Some(RepositoryLinkKind::Blame),
                        "commits" => Some(RepositoryLinkKind::Commits),
                        "compare" => Some(RepositoryLinkKind::Compare),
                        _ => None,
                    };
                    match repository_kind {
                        Some(kind) => (
                            segments[..marker].join("/"),
                            ParsedLinkKind::Repository {
                                kind,
                                locator: resource[1..].join("/"),
                            },
                            None,
                            None,
                        ),
                        None => (
                            segments[..marker].join("/"),
                            ParsedLinkKind::ProjectPage(resource.join("/")),
                            None,
                            None,
                        ),
                    }
                }
                None => (
                    segments[..marker].join("/"),
                    ParsedLinkKind::ProjectPage(resource.join("/")),
                    None,
                    None,
                ),
            }
        }
        _ => return None,
    };
    let comment_id = if matches!(&kind, ParsedLinkKind::MergeRequest | ParsedLinkKind::Issue) {
        fragment
            .and_then(|fragment| fragment.strip_prefix("note_"))
            .and_then(|id| id.parse().ok())
    } else {
        None
    };
    let line_selection = fragment.and_then(parse_line_selection);
    Some(GitlabLink {
        host: host.to_string(),
        project,
        kind,
        number,
        reference,
        comment_id,
        line_selection,
        ref_type,
    })
}

fn parse_line_selection(fragment: &str) -> Option<LineSelection> {
    let value = fragment.strip_prefix('L')?;
    match value.split_once('-') {
        None => {
            let line = value.parse().ok()?;
            (line > 0).then_some(LineSelection::Line(line))
        }
        Some((start, end)) => {
            let start = start.parse().ok()?;
            let end = end.strip_prefix('L').unwrap_or(end).parse().ok()?;
            (start > 0 && end >= start).then_some(LineSelection::Range { start, end })
        }
    }
}
