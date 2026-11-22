//! Network requests: connections a sandbox's programs tried that its policy doesn't allow. Playpen makes every sandbox
//! with approvals left to the person, so OpenShell turns each into a proposed rule, which waits until it's approved,
//! and the sandbox loads the new policy a few seconds later, or rejected, which it stays. `openshell rule get` gives
//! them only as text, read here.

use super::{OpenShell, OpenShellError};

/// Where a request stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestStatus {
    Pending,
    Approved,
    Rejected,
    /// One a newer OpenShell has that Playpen doesn't know.
    Other(String),
}

impl RequestStatus {
    fn of(status: &str) -> RequestStatus {
        match status {
            "pending" => RequestStatus::Pending,
            "approved" => RequestStatus::Approved,
            "rejected" => RequestStatus::Rejected,
            other => RequestStatus::Other(other.to_string()),
        }
    }
}

/// A rule OpenShell proposes for a connection that was blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRequest {
    /// What OpenShell knows it by, to approve or reject it.
    pub id: String,
    pub status: RequestStatus,
    /// The rule's name, such as `allow_github_com_443`.
    pub rule: String,
    /// The programs it would let through, by path.
    pub programs: Vec<String>,
    /// Where it would let them go: `host:port`, with OpenShell's note of how it's checked, as `github.com:443 [L4]`.
    pub endpoints: Vec<String>,
    /// How sure OpenShell is that it's wanted, out of 100.
    pub confidence: Option<u8>,
    /// Why OpenShell proposes it, in its words: only ever shown, since what's granted is the programs and endpoints.
    pub rationale: String,
}

impl NetworkRequest {
    pub fn is_pending(&self) -> bool {
        self.status == RequestStatus::Pending
    }
}

/// The requests in what `openshell rule get` printed: a block for each, its lines `Key: value`, each block starting
/// with `Chunk:`. Lines it doesn't know are passed over, so a newer OpenShell's are no trouble.
pub(super) fn parse_requests(printed: &str) -> Vec<NetworkRequest> {
    let mut requests: Vec<NetworkRequest> = Vec::new();
    for line in printed.lines() {
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let value = value.trim();
        if key == "Chunk" {
            requests.push(NetworkRequest {
                id: value.to_string(),
                status: RequestStatus::Other(String::new()),
                rule: String::new(),
                programs: Vec::new(),
                endpoints: Vec::new(),
                confidence: None,
                rationale: String::new(),
            });
            continue;
        }
        let Some(request) = requests.last_mut() else {
            continue;
        };
        let list = |value: &str| -> Vec<String> {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        };
        match key {
            "Status" => request.status = RequestStatus::of(value),
            "Rule" => request.rule = value.to_string(),
            "Binaries" => request.programs = list(value),
            // `Binary` is the one that asked; `Binaries` is every one the rule names, so it's used when it's there.
            "Binary" if request.programs.is_empty() => request.programs = list(value),
            "Endpoints" => request.endpoints = list(value),
            "Confidence" => request.confidence = value.trim_end_matches('%').parse().ok(),
            "Rationale" => request.rationale = value.to_string(),
            _ => {}
        }
    }
    requests
}

impl OpenShell {
    /// The network requests of the sandbox `name`, waiting and decided.
    pub fn network_requests(&self, name: &str) -> Result<Vec<NetworkRequest>, OpenShellError> {
        let printed = self.run(
            &format!("list the network requests of “{name}”"),
            &["rule", "get", name],
        )?;
        Ok(parse_requests(&printed))
    }

    /// Approves the request `id` of the sandbox `name`. The sandbox loads the new policy a few seconds later.
    pub fn approve_request(&self, name: &str, id: &str) -> Result<(), OpenShellError> {
        self.run(
            &format!("approve a network request of “{name}”"),
            &["rule", "approve", name, "--chunk-id", id],
        )
        .map(drop)
    }

    /// Rejects the request `id` of the sandbox `name`, which stays rejected when the same connection is tried again.
    pub fn reject_request(&self, name: &str, id: &str) -> Result<(), OpenShellError> {
        self.run(
            &format!("reject a network request of “{name}”"),
            &["rule", "reject", name, "--chunk-id", id],
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::{RequestStatus, parse_requests};

    /// What OpenShell 0.1.9 printed for a sandbox whose node and git were each blocked once, one since approved.
    const PRINTED: &str = "Network Rules:  (version 1, 2 chunks)

  Chunk: 8d2b3223-c829-4891-b3b7-91fa5c259b89
  Status: pending
  Rule: allow_github_com_443
  Binary: /usr/lib/git-core/git-remote-http
  Confidence: 65%
  Rationale: Allow git-remote-http to connect to github.com:443 (HTTPS).
  Prover: prover: no new findings
  Candidate: a5bdce73ce44
  Endpoints: github.com:443 [L4]
  Binaries: /usr/lib/git-core/git-remote-http

  Chunk: 60773847-b0e1-4ead-8793-f568be7f8155
  Status: approved
  Rule: allow_example_com_443
  Binary: /usr/local/bin/node
  Confidence: 65%
  Rationale: Allow node to connect to example.com:443 (HTTPS).
  Prover: prover: no new findings
  Candidate: 21f7689caede
  Endpoints: example.com:443 [L4]
  Binaries: /usr/local/bin/node
";

    #[test]
    fn each_request_is_read_with_what_it_would_let_through() {
        let requests = parse_requests(PRINTED);
        insta::assert_debug_snapshot!(requests);
        assert!(requests[0].is_pending());
        assert_eq!(requests[1].status, RequestStatus::Approved);
    }

    #[test]
    fn a_sandbox_with_no_requests_has_none() {
        assert!(parse_requests("No network rules for sandbox 'claude-patchbay'\n").is_empty());
    }

    #[test]
    fn a_status_playpen_doesnt_know_is_kept_as_it_was_said() {
        let requests = parse_requests("  Chunk: abc\n  Status: superseded\n  Rule: allow_x\n");
        assert_eq!(
            requests[0].status,
            RequestStatus::Other("superseded".to_string())
        );
    }
}
