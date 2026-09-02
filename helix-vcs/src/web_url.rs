//! Turning a git remote into a link you can paste into a browser.
//!
//! Every forge agrees on the host and the repository path but disagrees on
//! everything after it (`/-/blob/` on GitLab, `/blob/` on GitHub,
//! `/src/commit/` on Gitea, query parameters on Azure DevOps), so the shapes
//! live in a small per-host table that the user can override in the config.
//!
//! Azure DevOps is matched on its https remote (`dev.azure.com`); its ssh
//! remotes live on `ssh.dev.azure.com` under a different path layout, so those
//! fall through to the generic shape and need a `url-template` override.
//!
//! This module is deliberately free of any VCS dependency: it takes the remote
//! URL as the string git stored and does its own light parsing, so it compiles
//! and tests without the `git` feature.

/// The pieces of a remote URL that are needed to address a repository on the web.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebBase {
    /// Host, lowercased, including a port when the remote itself was http(s).
    pub host: String,
    /// Repository path within the host, without a leading slash or `.git` suffix.
    pub repo: String,
    /// `scheme://host[:port]/repo`, the prefix every template starts from.
    pub base: String,
}

/// The provider-specific URL shapes. `file` addresses the file at a commit; the
/// two line forms are appended depending on whether the selection covers one
/// line or several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebUrlTemplate {
    pub file: String,
    pub line: String,
    pub line_range: String,
}

/// Everything a template can interpolate apart from the line numbers.
#[derive(Debug, Clone)]
pub struct WebUrlFields<'a> {
    pub base: &'a WebBase,
    pub commit: &'a str,
    /// Repository-root relative, forward slashes, not yet percent-encoded.
    pub path: &'a str,
}

const GITLAB: (&str, &str, &str) = (
    "{base}/-/blob/{commit}/{path}",
    "#L{line}",
    "#L{line}-{end-line}",
);
const GITHUB: (&str, &str, &str) = (
    "{base}/blob/{commit}/{path}",
    "#L{line}",
    "#L{line}-L{end-line}",
);
const GITEA: (&str, &str, &str) = (
    "{base}/src/commit/{commit}/{path}",
    "#L{line}",
    "#L{line}-L{end-line}",
);
const BITBUCKET: (&str, &str, &str) = (
    "{base}/src/{commit}/{path}",
    "#lines-{line}",
    "#lines-{line}:{end-line}",
);
const AZURE: (&str, &str, &str) = (
    "{base}?path=/{path}&version=GC{commit}",
    "&line={line}",
    "&line={line}&lineEnd={end-line}",
);

impl WebUrlTemplate {
    fn from_parts((file, line, line_range): (&str, &str, &str)) -> Self {
        Self {
            file: file.to_string(),
            line: line.to_string(),
            line_range: line_range.to_string(),
        }
    }
}

/// Pick the URL shape for a host. Unrecognized hosts get the GitLab shape,
/// which is the common case for self-hosted forges; override it with
/// `[editor.git-remote] url-template` when it guesses wrong.
pub fn template_for_host(host: &str) -> WebUrlTemplate {
    let host = host.to_ascii_lowercase();
    // Strip a port so `gitlab.example.com:8443` still matches.
    let bare = host.split(':').next().unwrap_or(&host);

    let parts = if bare.contains("github") {
        GITHUB
    } else if bare.contains("gitlab") {
        GITLAB
    } else if bare == "bitbucket.org" || bare.ends_with(".bitbucket.org") {
        BITBUCKET
    } else if bare == "codeberg.org" || bare.contains("gitea") || bare.contains("forgejo") {
        GITEA
    } else if bare == "dev.azure.com" || bare.ends_with(".visualstudio.com") {
        AZURE
    } else {
        GITLAB
    };

    WebUrlTemplate::from_parts(parts)
}

/// Parse a git remote URL into the pieces a web link needs.
///
/// Handles the three forms git accepts — `scheme://[user@]host[:port]/path`,
/// scp-like `[user@]host:path`, and plain local paths — and returns `None` for
/// anything that has no host to browse (local paths, `file://`).
pub fn web_base(remote: &str) -> Option<WebBase> {
    let remote = remote.trim();
    if remote.is_empty() {
        return None;
    }

    let (scheme, authority, path) = match remote.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            (scheme.to_ascii_lowercase(), authority, path)
        }
        None => {
            // scp-like `git@host:group/repo.git`. A leading `/`, `./` or a
            // Windows drive letter means a local path instead.
            if remote.starts_with('/') || remote.starts_with('.') {
                return None;
            }
            let (authority, path) = remote.split_once(':')?;
            if authority.len() <= 1 {
                // A single character before the colon is a Windows drive.
                return None;
            }
            ("ssh".to_string(), authority, path)
        }
    };

    if scheme == "file" {
        return None;
    }

    // Drop userinfo; `git@host` and `host` browse the same place.
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // An ssh port says nothing about the web port, so keep one only when the
    // remote was already http(s).
    let host = match host.rsplit_once(':') {
        Some((h, port))
            if scheme != "http"
                && scheme != "https"
                && !port.is_empty()
                && port.bytes().all(|b| b.is_ascii_digit()) =>
        {
            h
        }
        _ => host,
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }

    let repo = path.trim_matches('/');
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let repo = repo.trim_matches('/').to_string();
    if repo.is_empty() {
        return None;
    }

    let web_scheme = if scheme == "http" { "http" } else { "https" };
    let base = format!("{web_scheme}://{host}/{repo}");

    Some(WebBase { host, repo, base })
}

/// Render a template, appending the line fragment when `lines` is given.
/// `lines` is a 1-based inclusive `(start, end)`; equal values use the
/// single-line form.
pub fn render(
    template: &WebUrlTemplate,
    fields: &WebUrlFields,
    lines: Option<(usize, usize)>,
) -> String {
    let fragment = match lines {
        Some((start, end)) if end > start => template.line_range.as_str(),
        Some(_) => template.line.as_str(),
        None => "",
    };

    let (line, end_line) = match lines {
        Some((start, end)) => (start.to_string(), end.to_string()),
        None => (String::new(), String::new()),
    };

    let encoded_path = encode_path(fields.path);
    let short_commit = fields.commit.get(..8).unwrap_or(fields.commit).to_string();

    let mut out = String::with_capacity(template.file.len() + fragment.len() + 64);
    for source in [template.file.as_str(), fragment] {
        expand(
            source,
            &mut out,
            &[
                ("{base}", &fields.base.base),
                ("{host}", &fields.base.host),
                ("{repo}", &fields.base.repo),
                ("{commit}", fields.commit),
                ("{short-commit}", &short_commit),
                ("{path}", &encoded_path),
                ("{line}", &line),
                ("{end-line}", &end_line),
            ],
        );
    }
    out
}

/// Substitute `{placeholder}` runs in `source`, appending to `out`. Unknown
/// placeholders are copied through verbatim so a typo in a user template is
/// visible in the result rather than silently swallowed.
fn expand(source: &str, out: &mut String, vars: &[(&str, &str)]) {
    let mut rest = source;
    'outer: while !rest.is_empty() {
        match rest.find('{') {
            None => {
                out.push_str(rest);
                break;
            }
            Some(start) => {
                out.push_str(&rest[..start]);
                rest = &rest[start..];
                for (name, value) in vars {
                    if let Some(tail) = rest.strip_prefix(name) {
                        out.push_str(value);
                        rest = tail;
                        continue 'outer;
                    }
                }
                out.push('{');
                rest = &rest['{'.len_utf8()..];
            }
        }
    }
}

/// Percent-encode a repository path for use in a URL, leaving the separators
/// and the unreserved set alone.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod test {
    use super::*;

    fn base(remote: &str) -> WebBase {
        web_base(remote).unwrap_or_else(|| panic!("no web base for {remote}"))
    }

    fn link(remote: &str, commit: &str, path: &str, lines: Option<(usize, usize)>) -> String {
        let base = base(remote);
        let template = template_for_host(&base.host);
        let fields = WebUrlFields {
            base: &base,
            commit,
            path,
        };
        render(&template, &fields, lines)
    }

    #[test]
    fn parses_every_remote_form() {
        assert_eq!(
            base("git@gitlab.example.com:grp/sub/repo.git").base,
            "https://gitlab.example.com/grp/sub/repo"
        );
        assert_eq!(
            base("ssh://git@gitlab.example.com:2222/grp/repo.git").base,
            "https://gitlab.example.com/grp/repo"
        );
        assert_eq!(
            base("https://user@github.com/owner/repo.git").base,
            "https://github.com/owner/repo"
        );
        assert_eq!(
            base("git://github.com/owner/repo").base,
            "https://github.com/owner/repo"
        );
        // An http remote keeps its scheme and its port.
        assert_eq!(
            base("http://gitlab.internal:8080/grp/repo.git").base,
            "http://gitlab.internal:8080/grp/repo"
        );
        assert_eq!(base("HTTPS://GitHub.COM/Owner/Repo.git").host, "github.com");
        // `.git` only goes away as a suffix.
        assert_eq!(base("git@host:grp/repo.gitlab.git").repo, "grp/repo.gitlab");
    }

    #[test]
    fn rejects_remotes_with_nothing_to_browse() {
        assert_eq!(web_base("file:///srv/git/repo.git"), None);
        assert_eq!(web_base("/srv/git/repo.git"), None);
        assert_eq!(web_base("../sibling.git"), None);
        assert_eq!(web_base("C:/repos/thing"), None);
        assert_eq!(web_base("https://github.com/"), None);
        assert_eq!(web_base(""), None);
    }

    #[test]
    fn known_hosts_get_their_own_shape() {
        assert_eq!(
            link(
                "git@github.com:o/r.git",
                "9f3c1ab",
                "src/main.rs",
                Some((42, 42))
            ),
            "https://github.com/o/r/blob/9f3c1ab/src/main.rs#L42"
        );
        assert_eq!(
            link(
                "git@gitlab.com:o/r.git",
                "9f3c1ab",
                "src/main.rs",
                Some((42, 58))
            ),
            "https://gitlab.com/o/r/-/blob/9f3c1ab/src/main.rs#L42-58"
        );
        assert_eq!(
            link(
                "git@github.com:o/r.git",
                "9f3c1ab",
                "src/main.rs",
                Some((42, 58))
            ),
            "https://github.com/o/r/blob/9f3c1ab/src/main.rs#L42-L58"
        );
        assert_eq!(
            link("git@bitbucket.org:o/r.git", "9f3c1ab", "a.rs", Some((4, 9))),
            "https://bitbucket.org/o/r/src/9f3c1ab/a.rs#lines-4:9"
        );
        assert_eq!(
            link("git@codeberg.org:o/r.git", "9f3c1ab", "a.rs", Some((4, 4))),
            "https://codeberg.org/o/r/src/commit/9f3c1ab/a.rs#L4"
        );
        assert_eq!(
            link(
                "https://org@dev.azure.com/org/proj/_git/repo",
                "9f3c1ab",
                "a.rs",
                Some((4, 9))
            ),
            "https://dev.azure.com/org/proj/_git/repo?path=/a.rs&version=GC9f3c1ab&line=4&lineEnd=9"
        );
    }

    #[test]
    fn unknown_hosts_fall_back_to_gitlab() {
        assert_eq!(
            link(
                "git@git.internal.example:team/tool.git",
                "abc1234",
                "x/y.rs",
                None
            ),
            "https://git.internal.example/team/tool/-/blob/abc1234/x/y.rs"
        );
    }

    #[test]
    fn paths_are_percent_encoded() {
        assert_eq!(
            link("git@github.com:o/r.git", "abc", "dir/a file.rs", None),
            "https://github.com/o/r/blob/abc/dir/a%20file.rs"
        );
    }

    #[test]
    fn custom_templates_see_every_placeholder() {
        let base = base("git@gitlab.example.com:grp/repo.git");
        let template = WebUrlTemplate {
            file: "{host}|{repo}|{commit}|{short-commit}|{path}|{unknown}".to_string(),
            line: "|{line}".to_string(),
            line_range: "|{line}..{end-line}".to_string(),
        };
        let fields = WebUrlFields {
            base: &base,
            commit: "0123456789abcdef",
            path: "a.rs",
        };
        assert_eq!(
            render(&template, &fields, Some((3, 7))),
            "gitlab.example.com|grp/repo|0123456789abcdef|01234567|a.rs|{unknown}|3..7"
        );
        assert_eq!(
            render(&template, &fields, None),
            "gitlab.example.com|grp/repo|0123456789abcdef|01234567|a.rs|{unknown}"
        );
    }
}
