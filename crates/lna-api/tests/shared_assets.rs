//! The lecture/slide player page is shared by every client (apps/shared/deck_player.html). Windows
//! and Android build it in from there; the macOS (SwiftPM resource) and Linux (include_str!)
//! targets need it inside their own folder, so they keep a copy. The copies must not drift.
use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

#[test]
fn every_client_ships_the_same_player_page() {
    let shared = std::fs::read(repo("apps/shared/deck_player.html")).expect("shared page");
    for copy in [
        "apps/macos/LocalNotebook/Sources/LocalNotebook/Resources/deck_player.html",
        "apps/linux/local-notebook-gtk/assets/deck_player.html",
    ] {
        let bytes = std::fs::read(repo(copy)).unwrap_or_else(|_| panic!("{copy} is missing"));
        assert!(
            bytes == shared,
            "{copy} differs from apps/shared/deck_player.html; copy the shared file over it"
        );
    }
}

#[test]
fn the_player_page_names_no_template_token_in_its_comment() {
    // every occurrence of a token is replaced by the hosts, so a token in the leading comment
    // would receive the whole lecture payload and could end the comment early
    let page = std::fs::read_to_string(repo("apps/shared/deck_player.html")).unwrap();
    let head = page.split("-->").next().unwrap_or("");
    assert!(
        !head.contains("__PAYLOAD__")
            && !head.contains("__LANG__")
            && !head.contains("__LECTURE__"),
        "the leading comment of deck_player.html must not mention template tokens"
    );
}
