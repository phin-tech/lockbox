//! Network requests on a real OpenShell gateway: a connection the sandbox's policy doesn't allow becomes a request,
//! which approving lets through and rejecting doesn't. It needs OpenShell's gateway, Docker and the internet, so it's
//! left out of `cargo test`: `just requests-test`.

use std::time::{Duration, Instant};

use playpen_core::{Agent, Docker, OpenShell, Phase, Plan, Reached, RequestStatus};

/// Asks for `host` from node in the sandbox `name`, and says what came back: a status, or `blocked`.
fn fetch(openshell: &OpenShell, name: &str, host: &str) -> String {
    let script = format!(
        "node -e 'fetch(\"https://{host}\").then(r => console.log(r.status)).catch(() => console.log(\"blocked\"))'"
    );
    let printed = openshell
        .exec_bytes(name, "fetch", &script, &[])
        .unwrap_or_default();
    String::from_utf8_lossy(&printed).trim().to_string()
}

/// What `url` on this Mac gives, or nothing.
fn get(url: &str) -> String {
    std::process::Command::new("/usr/bin/curl")
        .args(["-s", "--max-time", "3", url])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

/// Waits up to `seconds` for `done` to say yes.
fn within(seconds: u64, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    false
}

#[test]
#[ignore = "needs OpenShell's gateway, Docker and the internet"]
fn a_blocked_connection_waits_as_a_request_and_gets_through_once_approved() {
    let openshell = OpenShell::find().unwrap();
    let plan = Plan {
        name: format!("pp-requests-{}", std::process::id()),
        agent: Agent::Codex,
        provider: None,
        model: None,
        account: None,
        services: Vec::new(),
        variables: Vec::new(),
        setups: Vec::new(),
        folders: Vec::new(),
    };
    let name = plan.name.clone();
    // Everything is tried before anything is checked, so the sandbox is deleted whatever fails.
    let tried = (|| {
        plan.make(&openshell, &Docker::find().unwrap(), |_| {})
            .map_err(|error| error.to_string())?;
        let ready = within(90, || {
            openshell.sandboxes().is_ok_and(|sandboxes| {
                sandboxes
                    .iter()
                    .any(|sandbox| sandbox.name == name && sandbox.phase == Phase::Ready)
            })
        });
        if !ready {
            return Err("never ready".to_string());
        }
        let before = fetch(&openshell, &name, "example.com");
        let mut waiting = None;
        within(60, || {
            waiting = openshell
                .network_requests(&name)
                .ok()
                .and_then(|requests| requests.into_iter().find(|request| request.is_pending()));
            waiting.is_some()
        });
        let request = waiting.ok_or("no request appeared")?;
        openshell
            .approve_request(&name, &request.id)
            .map_err(|error| error.to_string())?;
        let mut after = String::new();
        within(60, || {
            after = fetch(&openshell, &name, "example.com");
            after == "200"
        });
        let status = openshell
            .network_requests(&name)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|found| found.id == request.id)
            .map(|found| found.status);
        // What the sandbox's log says it reached: example.com stopped, then let through by the approved rule.
        // What the sandbox's log says it reached: example.com stopped, then let through by the approved rule, which
        // reaches the gateway's log a few seconds after the connection.
        let let_through = |activity: &[playpen_core::Activity]| {
            activity.iter().any(|activity| {
                activity.allowed
                    && matches!(&activity.reached, Reached::Connection { host, .. } if host == "example.com")
            })
        };
        let mut activity = Vec::new();
        within(30, || {
            activity = openshell.activity(&name).unwrap_or_default();
            let_through(&activity)
        });
        // A web server in the sandbox, reached from this Mac through a forwarded port, then the forward stopped.
        openshell
            .exec_bytes(
                &name,
                "serve",
                "nohup node -e 'require(\"http\").createServer((q, s) => s.end(\"from the sandbox\")).listen(3917, \"127.0.0.1\")' >/tmp/web.log 2>&1 &",
                &[],
            )
            .map_err(|error| error.to_string())?;
        std::thread::sleep(Duration::from_secs(2));
        openshell
            .forward_port(&name, 3917)
            .map_err(|error| error.to_string())?;
        let mut served = String::new();
        within(20, || {
            served = get("http://127.0.0.1:3917/");
            served == "from the sandbox"
        });
        let listed = openshell.forwards().map_err(|error| error.to_string())?;
        openshell
            .stop_forward(&name, 3917)
            .map_err(|error| error.to_string())?;
        let closed = within(10, || get("http://127.0.0.1:3917/").is_empty());
        Ok::<_, String>((
            before, request, after, status, activity, served, listed, closed,
        ))
    })();
    let _ = openshell.delete(&name);
    let (before, request, after, status, activity, served, listed, closed) = tried.unwrap();
    assert_eq!(served, "from the sandbox", "the forwarded port reaches it");
    assert!(
        listed
            .iter()
            .any(|forward| forward.port == 3917 && forward.sandbox == name)
    );
    assert!(closed, "stopping closes the port");
    let to_example = |allowed: bool| {
        activity.iter().any(|activity| {
            activity.allowed == allowed
                && matches!(&activity.reached, Reached::Connection { host, .. } if host == "example.com")
        })
    };
    assert!(to_example(false), "stopped first: {activity:?}");
    assert!(to_example(true), "then let through: {activity:?}");
    assert_eq!(before, "blocked");
    assert!(
        request
            .endpoints
            .iter()
            .any(|endpoint| endpoint.starts_with("example.com:443")),
        "{request:?}"
    );
    assert!(
        request
            .programs
            .iter()
            .any(|program| program.ends_with("/node"))
    );
    assert_eq!(after, "200", "it gets through once approved");
    assert_eq!(status, Some(RequestStatus::Approved));
}
