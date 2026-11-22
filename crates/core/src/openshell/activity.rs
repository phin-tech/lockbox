//! A sandbox's recent network activity: each connection its programs made or were stopped from making, and each HTTP
//! request OpenShell looked inside, from the audit lines in its log. The gateway keeps the log in memory, so it goes
//! back only as far as the gateway's last start.

use super::{OpenShell, OpenShellError};

/// How many of the log's last lines are read: plenty for what a person scrolls through.
const LINES: &str = "2000";

/// What a sandbox reached, or tried to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reached {
    /// A connection from a program, by its path, to a host and port.
    Connection {
        program: String,
        host: String,
        port: String,
    },
    /// An HTTP request OpenShell looked inside, with the address it went to, less any query, which may hold a secret.
    Request { method: String, url: String },
}

/// One thing a sandbox reached, or was stopped from reaching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// When, in milliseconds since 1970.
    pub at_ms: i64,
    pub allowed: bool,
    pub reached: Reached,
    /// The rule that let it through, or why it was stopped.
    pub because: Option<String>,
}

impl Activity {
    /// When it was, as the time of day on this Mac: `14:02:11`.
    pub fn time_of_day(&self) -> String {
        jiff::Timestamp::from_millisecond(self.at_ms)
            .map(|at| {
                at.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%H:%M:%S")
                    .to_string()
            })
            .unwrap_or_default()
    }
}

/// `url` without its query or fragment.
fn without_query(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// The value of `[key:value …]` in what follows an audit line's verdict, if it's there.
fn tagged(rest: &str, keys: &[&str]) -> Option<String> {
    let inside = rest.rsplit_once('[')?.1.trim_end_matches(']');
    inside.split_whitespace().find_map(|pair| {
        let (key, value) = pair.split_once(':')?;
        keys.contains(&key).then(|| value.to_string())
    })
}

/// One line of the log, if it's an audit line saying something was allowed or denied: `NET:OPEN` for a connection,
/// `HTTP:<method>` for a request. Others, such as the supervisor's own connections and SSH sessions, aren't.
fn parse_line(line: &str) -> Option<Activity> {
    let at = line.strip_prefix('[')?.split_once(']')?.0;
    let seconds: f64 = at.parse().ok()?;
    let (_, event) = line.split_once("[ocsf] ")?;
    let (kind, rest) = event.split_once(' ')?;
    // The severity, such as `[INFO]`, then the verdict.
    let rest = rest.split_once("] ")?.1;
    let (allowed, rest) = match rest.strip_prefix("ALLOWED ") {
        Some(rest) => (true, rest),
        None => (false, rest.strip_prefix("DENIED ")?),
    };
    let because = if allowed {
        tagged(rest, &["policy"])
    } else {
        tagged(rest, &["reason"])
    };
    let reached = if kind == "NET:OPEN" {
        // `/usr/local/bin/node(0) -> example.com:443 [policy:… engine:opa]`
        let (program, rest) = rest.split_once(" -> ")?;
        let target = rest.split_whitespace().next()?;
        let (host, port) = target.rsplit_once(':')?;
        let program = program.split_once('(').map_or(program, |(path, _)| path);
        Reached::Connection {
            program: program.to_string(),
            host: host.to_string(),
            port: port.to_string(),
        }
    } else {
        // `GET https://example.com/`
        let method = kind.strip_prefix("HTTP:")?;
        let mut words = rest.split_whitespace();
        let _method = words.next()?;
        let url = words.next()?;
        Reached::Request {
            method: method.to_string(),
            url: without_query(url).to_string(),
        }
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a time in seconds since 1970, with milliseconds, fits an i64 of milliseconds"
    )]
    let at_ms = (seconds * 1000.0).round() as i64;
    Some(Activity {
        at_ms,
        allowed,
        reached,
        because,
    })
}

/// The activity in what `openshell logs` printed, oldest first.
pub(super) fn parse_activity(printed: &str) -> Vec<Activity> {
    printed.lines().filter_map(parse_line).collect()
}

impl OpenShell {
    /// The recent network activity of the sandbox `name`, oldest first, as far back as the gateway's last start.
    pub fn activity(&self, name: &str) -> Result<Vec<Activity>, OpenShellError> {
        let printed = self.run(
            &format!("read the activity of “{name}”"),
            &["logs", name, "-n", LINES, "--source", "sandbox"],
        )?;
        Ok(parse_activity(&printed))
    }
}

#[cfg(test)]
mod tests {
    use super::{Reached, parse_activity};

    /// What OpenShell 0.1.9 logged as a sandbox's node was stopped from reaching example.com, git from github.com,
    /// and node let through once a rule was approved, with the lines around them.
    const LOGGED: &str = "[1795375089.291] [sandbox] [OCSF ] [ocsf] NET:LISTEN [INFO] 127.0.0.1:3128
[1795375089.302] [sandbox] [OCSF ] [ocsf] NET:OPEN [INFO] 127.0.0.1:17670
[1795375089.393] [sandbox] [OCSF ] [ocsf] SSH:OPEN [INFO] ALLOWED
[1795375089.448] [sandbox] [OCSF ] [ocsf] NET:REFUSE [MED] DENIED example.com [reason:policy_dns_ineligible]
[1795375089.504] [sandbox] [WARN ] [openshell_supervisor_network::proxy] Denied staged transparent connection destination=198.18.0.2:443 reason=network connections not allowed by policy
[1795375089.504] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED /usr/local/bin/node(0) -> example.com:443 [reason:transparent_tcp_policy_denied]
[1795375089.524] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED /usr/lib/git-core/git-remote-http(0) -> github.com:443 [reason:transparent_tcp_policy_denied]
[1795375224.350] [sandbox] [OCSF ] [ocsf] NET:OPEN [INFO] ALLOWED /usr/local/bin/node(0) -> example.com:443 [policy:allow_example_com_443 engine:opa]
[1795375224.464] [sandbox] [OCSF ] [ocsf] HTTP:GET [INFO] ALLOWED GET https://example.com/search?q=secret-token
";

    #[test]
    fn connections_and_requests_are_read_with_why_and_nothing_else_is() {
        insta::assert_debug_snapshot!(parse_activity(LOGGED));
    }

    #[test]
    fn a_requests_query_is_left_out_since_it_may_hold_a_secret() {
        let activity = parse_activity(LOGGED);
        let Some(Reached::Request { url, .. }) = activity.last().map(|last| &last.reached) else {
            panic!("the last is a request");
        };
        assert_eq!(url, "https://example.com/search");
    }
}
