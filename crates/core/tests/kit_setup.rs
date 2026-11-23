//! A kit's setup folder on a real OpenShell gateway: its `install.sh` puts `jq` into the image, its `.bashrc` is linked
//! into the sandbox's home, and its `.npmrc`, which usually holds a secret, isn't. It needs OpenShell's gateway,
//! Docker and the internet, so it's left out of `cargo test`: `just kit-setup-test`.

use std::time::{Duration, Instant};

use playpen_core::{Agent, Docker, OpenShell, Phase, Plan, SetupFolder};

#[test]
#[ignore = "needs OpenShell's gateway, Docker and the internet"]
fn a_kits_install_sh_and_dotfiles_reach_the_sandbox_but_not_its_secrets() {
    let openshell = OpenShell::find().unwrap();
    let docker = Docker::find().unwrap();
    let pid = std::process::id();
    let folder = std::env::temp_dir().join(format!("pp-kit-{pid}"));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("install.sh"),
        "set -e\napt-get update -qq\napt-get install -y -qq --no-install-recommends jq >/dev/null\n",
    )
    .unwrap();
    std::fs::write(folder.join(".bashrc"), "export FROM_KIT=yes\n").unwrap();
    std::fs::write(
        folder.join(".npmrc"),
        "//registry.npmjs.org/:_authToken=secret\n",
    )
    .unwrap();
    let plan = Plan {
        name: format!("pp-kit-{pid}"),
        agent: Agent::ClaudeCode.into(),
        image: None,
        provider: None,
        model: None,
        account: None,
        services: Vec::new(),
        variables: Vec::new(),
        setups: vec![SetupFolder {
            kit: "tools".to_string(),
            folder: folder.clone(),
        }],
        folders: Vec::new(),
    };
    let image = plan.image();
    let tried = plan
        .make(&openshell, &docker, |_| {})
        .map_err(|error| error.to_string())
        .and_then(|()| {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !openshell
                .sandboxes()
                .unwrap()
                .iter()
                .any(|sandbox| sandbox.name == plan.name && sandbox.phase == Phase::Ready)
            {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_secs(2));
            }
            openshell
                .exec_bytes(
                    &plan.name,
                    "look",
                    "echo '{\"a\":1}' | jq -c .; . /sandbox/.bashrc; echo $FROM_KIT; ls -a /sandbox | grep -c npmrc || true",
                    &[],
                )
                .map(|said| String::from_utf8_lossy(&said).into_owned())
                .map_err(|error| error.to_string())
        });
    let _ = openshell.delete(&plan.name);
    let _ = std::fs::remove_dir_all(&folder);
    assert!(image.starts_with("playpen-claude-code-kits:"), "{image}");
    assert_eq!(tried.unwrap(), "{\"a\":1}\nyes\n0\n");
}
