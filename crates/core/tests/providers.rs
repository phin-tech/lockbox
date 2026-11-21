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
