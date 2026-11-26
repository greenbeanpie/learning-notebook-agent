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

fn katex_block() -> String {
    let page = std::fs::read_to_string(repo("apps/shared/deck_player.html")).unwrap();
    let a = page
        .find("<!-- KATEX:BEGIN -->")
        .expect("KATEX:BEGIN marker missing");
    let b = page
        .find("<!-- KATEX:END -->")
        .expect("KATEX:END marker missing");
    assert!(a < b, "KATEX markers out of order");
    page[a..b].to_string()
}

#[test]
fn the_player_page_embeds_katex_with_its_fonts() {
    let block = katex_block();
    assert!(block.contains("<style>") && block.contains("<script>"));
    assert!(
        block.matches("@font-face").count() >= 8,
        "the KaTeX block needs its embedded @font-face rules"
    );
    assert!(
        !block.contains("url(fonts/"),
        "a font file reference is left in the embedded KaTeX CSS"
    );
}

#[test]
fn the_katex_block_has_no_template_token() {
    // the hosts replace tokens such as the payload placeholder everywhere in the page
    let block = katex_block();
    let bytes = block.as_bytes();
    let mut i = 0;
    while let Some(p) = block[i..].find("__") {
        let start = i + p + 2;
        let mut j = start;
        while j < bytes.len() && (bytes[j].is_ascii_uppercase() || bytes[j] == b'_') {
            j += 1;
        }
        let is_token = j > start
            && bytes[start].is_ascii_uppercase()
            && block[start..j].ends_with("__")
            && j - start > 2;
        assert!(
            !is_token,
            "template-like token {:?} inside the KaTeX block",
            &block[start - 2..j]
        );
        i = start;
    }
}

#[test]
fn the_katex_script_never_closes_the_script_element() {
    let block = katex_block();
    for chunk in block.split("<script>").skip(1) {
        let (js, _) = chunk.split_once("</script>").expect("unterminated script");
        assert!(
            !js.to_ascii_lowercase().contains("</script"),
            "a raw </script inside the KaTeX script text ends the element early"
        );
    }
}
