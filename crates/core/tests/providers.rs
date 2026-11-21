//! A LiteLLM provider added to a real OpenShell gateway, given a new key and deleted, with its profile. The key is
//! made up and the proxy's host doesn't exist, since nothing is sent to it.
//!
//! It needs OpenShell's gateway, so it's left out of `cargo test`:
//! `cargo test -p playpen-core --test providers -- --ignored`.

use playpen_core::{Key, Kind, NewProvider, OpenShell};

#[test]
#[ignore = "needs OpenShell's gateway"]
fn a_litellm_provider_is_added_given_a_new_key_and_deleted_with_its_profile() {
    let openshell = OpenShell::find().unwrap();
    let name = format!("playpen-test-{}", std::process::id());
    let new = NewProvider::new(Kind::LiteLlm, &name, "https://litellm.invalid:4000").unwrap();
    openshell
        .add_provider(&new, &Key::new("not-a-real-key"))
        .unwrap();
    let added = openshell
        .providers()
        .unwrap()
        .into_iter()
        .find(|provider| provider.name == name)
        .expect("listed");
    assert_eq!(added.playpen_kind(), Some(Kind::LiteLlm));
    let replaced = openshell.replace_key(&added, &Key::new("another-made-up-key"));
    let deleted = openshell.delete_provider(&added);
    replaced.unwrap();
    deleted.unwrap();
    assert!(
        !openshell
            .providers()
            .unwrap()
            .iter()
            .any(|provider| provider.name == name)
    );
    let profiles = std::process::Command::new(openshell.binary())
        .args(["provider", "list-profiles"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&profiles.stdout).contains(&new.profile_id()));
}

#[test]
#[ignore = "needs OpenShell's gateway"]
fn a_second_provider_of_a_kind_shares_its_profile_and_a_taken_name_is_refused() {
    let openshell = OpenShell::find().unwrap();
    let names = [1, 2].map(|n| format!("playpen-test-{}-{n}", std::process::id()));
    let added: Vec<_> = names
        .iter()
        .map(|name| {
            let new = NewProvider::new(Kind::OpenRouter, name, "").unwrap();
            openshell.add_provider(&new, &Key::new("not-a-real-key"))
        })
        .collect();
    let again = openshell.add_provider(
        &NewProvider::new(Kind::OpenRouter, &names[0], "").unwrap(),
        &Key::new("not-a-real-key"),
    );
    for provider in openshell.providers().unwrap() {
        if names.contains(&provider.name) {
            openshell.delete_provider(&provider).unwrap();
        }
    }
    for added in added {
        added.unwrap();
    }
    assert!(again.unwrap_err().to_string().contains("already"));
}

#[test]
#[ignore = "needs OpenShell's gateway"]
fn a_chatgpt_provider_is_added_renewable_signed_in_again_and_deleted() {
    use playpen_core::{Account, Tokens};

    let openshell = OpenShell::find().unwrap();
    let name = format!("playpen-test-{}-chatgpt", std::process::id());
    // Made up, and expiring days from now, so the gateway doesn't try the refresh token on OpenAI during the test.
    let tokens = |n: u8| Tokens {
        access: Key::new(&format!("made-up-access-{n}")),
        refresh: Key::new(&format!("made-up-refresh-{n}")),
        expires_at_ms: (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64)
            + 5 * 24 * 3600 * 1000,
        account: Account {
            account_id: "acct-test".to_string(),
            id_token: "x.y.".to_string(),
        },
    };
    let new = NewProvider::new(Kind::ChatGptSubscription, &name, "").unwrap();
    let added = openshell.add_signed_in(&new, &tokens(1));
    let listed = openshell
        .providers()
        .unwrap()
        .into_iter()
        .find(|provider| provider.name == name);
    let again = listed
        .as_ref()
        .map(|provider| openshell.sign_in_again(provider, &tokens(2)));
    let status = std::process::Command::new(openshell.binary())
        .args([
            "provider",
            "refresh",
            "status",
            &name,
            "--credential-key",
            Kind::ChatGptSubscription.variable(),
        ])
        .output()
        .unwrap();
    if let Some(provider) = &listed {
        openshell.delete_provider(provider).unwrap();
    }
    added.unwrap();
    again.expect("listed").unwrap();
    let status = String::from_utf8_lossy(&status.stdout).to_lowercase();
    assert!(status.contains("oauth2"), "renewal is set up: {status}");
}
