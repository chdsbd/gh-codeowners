//! Checks that ask GitHub's API, through `gh api`, about a rule's owners.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::Command;

use gh_codeowners::codeowners::{CodeownersRule, LineProblem};
use serde_json::Value;

/// A GET request to GitHub's API: the response, or None for a 404.
pub(crate) type Get<'a> = dyn Fn(&str) -> Result<Option<Value>, String> + Sync + 'a;

// How many requests to make at once.
const CONCURRENCY: usize = 8;

/// GET `endpoint` with gh, from `dir`, so `{owner}/{repo}` is the repository
/// gh finds there, or `GH_REPO`.
pub(crate) fn gh_get(dir: &Path, endpoint: &str) -> Result<Option<Value>, String> {
    let output = Command::new("gh")
        // Includes a team's permissions in a repository response.
        .args([
            "api",
            "-H",
            "Accept: application/vnd.github.v3.repository+json",
        ])
        .arg(endpoint)
        .current_dir(dir)
        .output()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => {
                "--online needs the GitHub CLI, gh: https://cli.github.com".to_string()
            }
            _ => format!("Couldn't run gh: {error}"),
        })?;
    let body = serde_json::from_slice::<Value>(&output.stdout);
    if output.status.success() {
        return body
            .map(Some)
            .map_err(|error| format!("gh api {endpoint}: {error}"));
    }
    // gh prints the error response, and exits with 1.
    if let Ok(body) = body
        && body["status"] == "404"
    {
        return Ok(None);
    }
    Err(format!(
        "gh api {endpoint}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// The repository's `owner/name`.
pub(crate) fn repository(get: &Get) -> Result<String, String> {
    get("repos/{owner}/{repo}")?
        .and_then(|repo| repo["full_name"].as_str().map(str::to_string))
        .ok_or_else(|| "GitHub repository not found".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeamAccess {
    Missing,
    Secret,
    None,
    Read,
    Write,
}

/// What `team`, like `acme/web`, can do in `repo`.
fn team_access(get: &Get, team: &str, repo: &str) -> Result<TeamAccess, String> {
    let (org, slug) = team.split_once('/').expect("a team is org/team");
    let Some(details) = get(&format!("orgs/{org}/teams/{slug}"))? else {
        return Ok(TeamAccess::Missing);
    };
    if details["privacy"] == "secret" {
        return Ok(TeamAccess::Secret);
    }
    // Includes access inherited from a parent team.
    let Some(repo) = get(&format!("orgs/{org}/teams/{slug}/repos/{repo}"))? else {
        return Ok(TeamAccess::None);
    };
    Ok(if repo["permissions"]["push"] == true {
        TeamAccess::Write
    } else {
        TeamAccess::Read
    })
}

/// An error for each team owner GitHub won't request a review from, because
/// it isn't visible or can't write to `repo`.
pub(crate) fn team_problems(
    rules: &[CodeownersRule],
    repo: &str,
    get: &Get,
) -> Result<Vec<LineProblem>, String> {
    let owners = || {
        rules.iter().flat_map(|rule| {
            rule.owners
                .iter()
                .zip(&rule.owner_spans)
                .map(move |(owner, span)| (rule, owner, span))
        })
    };
    // Teams are case insensitive, so ask about each once.
    let mut teams: Vec<String> = owners()
        .filter_map(|(_, owner, _)| owner.strip_prefix('@'))
        .filter(|owner| owner.contains('/'))
        .map(str::to_lowercase)
        .collect();
    teams.sort();
    teams.dedup();
    let mut access = HashMap::new();
    for chunk in teams.chunks(CONCURRENCY) {
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|team| scope.spawn(|| team_access(get, team, repo)))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("team request thread"))
                .collect::<Vec<_>>()
        });
        for (team, result) in chunk.iter().zip(results) {
            access.insert(team.as_str(), result?);
        }
    }
    Ok(owners()
        .filter_map(|(rule, owner, span)| {
            let team = owner.strip_prefix('@')?.to_lowercase();
            let message = match access.get(team.as_str())? {
                TeamAccess::Write => return None,
                TeamAccess::Missing => format!(
                    "`{owner}` doesn't exist, or you can't see it - code owner teams must exist and be visible"
                ),
                TeamAccess::Secret => {
                    format!("`{owner}` is a secret team - code owner teams must be visible")
                }
                TeamAccess::None => format!(
                    "`{owner}` has no access to {repo} - code owner teams must have write access"
                ),
                TeamAccess::Read => format!(
                    "`{owner}` can't write to {repo} - code owner teams must have write access"
                ),
            };
            Some(LineProblem {
                lineno: Some(rule.lineno),
                span: Some(span.clone()),
                error: true,
                message,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gh_codeowners::codeowners::parse_codeowners;
    use serde_json::json;

    use super::*;

    /// A fake API with these teams, as (team, privacy, push access to
    /// acme/app if any).
    fn api(teams: &[(&str, &str, Option<bool>)]) -> impl Fn(&str) -> Result<Option<Value>, String> {
        let teams: Vec<_> = teams
            .iter()
            .map(|&(team, privacy, push)| (team.to_string(), privacy.to_string(), push))
            .collect();
        move |endpoint| {
            Ok(teams.iter().find_map(|(team, privacy, push)| {
                let (org, slug) = team.split_once('/').unwrap();
                let team_endpoint = format!("orgs/{org}/teams/{slug}");
                if endpoint == team_endpoint {
                    Some(json!({ "privacy": privacy }))
                } else if endpoint == format!("{team_endpoint}/repos/acme/app") {
                    push.map(|push| json!({ "permissions": { "push": push } }))
                } else {
                    None
                }
            }))
        }
    }

    fn messages(text: &str, get: &Get) -> Vec<(usize, String, String)> {
        team_problems(&parse_codeowners(text), "acme/app", get)
            .unwrap()
            .into_iter()
            .map(|p| {
                let span = p.span.unwrap();
                (p.lineno.unwrap(), text[span].to_string(), p.message)
            })
            .collect()
    }

    #[test]
    fn reports_teams_github_wont_request_reviews_from() {
        let get = api(&[
            ("acme/web", "closed", Some(true)),
            ("acme/readers", "closed", Some(false)),
            ("acme/outsiders", "closed", None),
            ("acme/hidden", "secret", Some(true)),
        ]);
        let text = "* @acme/web @someone\n/a/ @acme/readers @acme/outsiders\n/b/ @acme/hidden @acme/gone dev@example.com\n";
        assert_eq!(
            messages(text, &get),
            [
                (
                    1,
                    "@acme/readers".to_string(),
                    "`@acme/readers` can't write to acme/app - code owner teams must have write access".to_string()
                ),
                (
                    1,
                    "@acme/outsiders".to_string(),
                    "`@acme/outsiders` has no access to acme/app - code owner teams must have write access".to_string()
                ),
                (
                    2,
                    "@acme/hidden".to_string(),
                    "`@acme/hidden` is a secret team - code owner teams must be visible".to_string()
                ),
                (
                    2,
                    "@acme/gone".to_string(),
                    "`@acme/gone` doesn't exist, or you can't see it - code owner teams must exist and be visible".to_string()
                ),
            ]
        );
    }

    #[test]
    fn asks_about_each_team_once_ignoring_case() {
        let requests = Mutex::new(Vec::new());
        let fake = api(&[("acme/web", "closed", Some(false))]);
        let get = |endpoint: &str| {
            requests.lock().unwrap().push(endpoint.to_string());
            fake(endpoint)
        };
        let problems = messages("/a/ @acme/web\n/b/ @Acme/Web\n", &get);
        assert_eq!(problems.len(), 2);
        let mut requests = requests.into_inner().unwrap();
        requests.sort();
        assert_eq!(
            requests,
            ["orgs/acme/teams/web", "orgs/acme/teams/web/repos/acme/app"]
        );
    }

    #[test]
    fn stops_at_an_api_error() {
        let get = |_: &str| Err("gh api: HTTP 401".to_string());
        assert_eq!(
            team_problems(&parse_codeowners("* @acme/web\n"), "acme/app", &get),
            Err("gh api: HTTP 401".to_string())
        );
    }
}
