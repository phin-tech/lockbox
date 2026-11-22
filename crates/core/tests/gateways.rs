//! A gateway registered with OpenShell and removed again, on its real configuration: the gateway that was active is
//! still active after it's added, and it's gone after it's removed.
//!
//! It adds a gateway whose address nothing answers (`http://127.0.0.1:9`), which OpenShell registers with a warning, and
//! it removes it whatever happens, so it's left out of `cargo test` all the same:
//! `cargo test -p playpen-core --test gateways -- --ignored`.

use playpen_core::{Forgotten, NewGateway, OpenShell};

const NAME: &str = "pp-add-test";

fn active(openshell: &OpenShell) -> Option<String> {
    openshell
        .gateways()
        .unwrap()
        .into_iter()
        .find(|gateway| gateway.active)
        .map(|gateway| gateway.name)
}

#[test]
#[ignore = "registers and removes a gateway in OpenShell's real configuration"]
fn a_gateway_is_added_without_becoming_active_and_removed_again() {
    let openshell = OpenShell::find().unwrap();
    let before = active(&openshell);
    let new = NewGateway::new("http://127.0.0.1:9", NAME).unwrap();

    let added = openshell.add_gateway(&new);
    let listed = openshell.gateways().unwrap();
    let found = listed.iter().find(|gateway| gateway.name == NAME).cloned();
    let after = active(&openshell);
    // Removed whatever happened, so a failure here doesn't leave it registered.
    let removed = found
        .as_ref()
        .map(|gateway| openshell.remove_gateway(gateway));
    let gone = openshell
        .gateways()
        .unwrap()
        .iter()
        .all(|gateway| gateway.name != NAME);

    added.unwrap();
    assert!(found.is_some(), "it's listed once it's added");
    assert_eq!(after, before, "the gateway that was active still is");
    assert_eq!(removed.unwrap().unwrap(), Forgotten::Clean);
    assert!(gone, "it's gone once it's removed");
    assert_eq!(active(&openshell), before);
}
