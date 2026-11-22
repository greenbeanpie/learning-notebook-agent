use crate::parser;
use lna_types::SourceKind;

pub const IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "ico", "jp2", "png", "webp", "tif", "tiff", "heic", "heif", "jpeg",
    "jpg", "jpe",
];

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aifc", "aiff", "amr", "au", "m4a", "mp3", "ogg", "opus", "ra", "snd", "wav",
    "wma",
];

pub const VIDEO_EXTENSIONS: &[&str] = &["3g2", "3gp", "avi", "mp4", "mpeg"];

pub fn classify_extension(ext: &str) -> Option<SourceKind> {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    match e.as_str() {
        "pdf" => Some(SourceKind::Pdf),
        "md" | "markdown" | "mdx" => Some(SourceKind::Md),
        "docx" => Some(SourceKind::Docx),
        "csv" => Some(SourceKind::Csv),
        "pptx" => Some(SourceKind::Pptx),
        "epub" => Some(SourceKind::Epub),
        "odt" => Some(SourceKind::Odt),
        "rtf" => Some(SourceKind::Rtf),
        "hwp" => Some(SourceKind::Hwp),
        "hwpx" => Some(SourceKind::Hwpx),
        s if IMAGE_EXTENSIONS.contains(&s) => Some(SourceKind::Image),
        s if AUDIO_EXTENSIONS.contains(&s) => Some(SourceKind::Audio),
        s if VIDEO_EXTENSIONS.contains(&s) => Some(SourceKind::Video),
        "txt" | "text" | "rst" | "org" | "log" | "json" | "jsonl" | "ndjson" | "yaml"
        | "yml" | "toml" | "xml" | "html" | "htm" | "xhtml" | "svg" | "tex" | "latex"
        | "ini" | "cfg" | "conf" | "tsv" | "sql" | "py" | "pyi" | "js" | "mjs" | "cjs"
        | "ts" | "tsx" | "jsx" | "rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "cs"
        | "fs" | "vb" | "java" | "kt" | "kts" | "go" | "rb" | "php" | "sh" | "bash"
        | "zsh" | "fish" | "ps1" | "psm1" | "bat" | "cmd" | "css" | "scss" | "sass"
        | "less" | "vue" | "svelte" | "astro" | "r" | "rmd" | "swift" | "scala" | "dart"
        | "lua" | "pl" | "pm" | "ex" | "exs" | "erl" | "hs" | "ml" | "mli" | "zig"
        | "nim" | "v" | "sv" | "vhd" | "env" | "properties" | "srt" | "vtt" | "lrc"
        | "ass" | "ssa" | "sub" | "bib" | "ris" | "graphql" | "gql" | "proto" | "gradle"
        | "cmake" | "makefile" | "dockerfile" | "gitignore" | "editorconfig" | "diff"
        | "patch" | "nfo" | "asc" | "asm" | "s" | "xaml" | "csproj" | "sln" => {
            Some(SourceKind::Txt)
        }
        _ => None,
    }
}

pub fn is_audio_or_video(kind: SourceKind) -> bool {
    matches!(kind, SourceKind::Audio | SourceKind::Video)
}

/// Checks whether arbitrary unknown-extension bytes look like human-readable plain text.
/// Handles UTF-8 and UTF-16 BOM files while rejecting binary payloads (NUL bytes / control soup).
pub fn is_probably_plain_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let sample = &bytes[..bytes.len().min(8192)];
    if let Some((enc, _)) = encoding_rs::Encoding::for_bom(sample) {
        let (cow, _, had_errors) = enc.decode(sample);
        if had_errors {
            return false;
        }
        return text_printable_ratio(&cow) >= 0.85;
    }
    if sample.contains(&0) {
        return false;
    }
    if let Ok(s) = std::str::from_utf8(sample) {
        return text_printable_ratio(s) >= 0.85;
    }
    let decoded = parser::decode_text_bytes(sample);
    !decoded.is_empty() && text_printable_ratio(&decoded) >= 0.85
}

fn text_printable_ratio(s: &str) -> f64 {
    let mut total = 0usize;
    let mut good = 0usize;
    for c in s.chars() {
        total += 1;
        if !c.is_control() || matches!(c, '\n' | '\r' | '\t') {
            good += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        good as f64 / total as f64
    }
}

pub fn mime_type_for_ext(ext: &str) -> &'static str {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    match e.as_str() {
        // Images
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "jp2" => "image/jp2",
        "png" => "image/png",
        "webp" => "image/webp",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "jpeg" | "jpg" | "jpe" => "image/jpeg",
        // Audio
        "aac" => "audio/aac",
        "aif" | "aifc" | "aiff" => "audio/aiff",
        "amr" => "audio/amr",
        "au" | "snd" => "audio/basic",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "ra" => "audio/vnd.rn-realaudio",
        "wav" => "audio/wav",
        "wma" => "audio/x-ms-wma",
        // Video
        "3g2" => "video/3gpp2",
        "3gp" => "video/3gpp",
        "avi" => "video/x-msvideo",
        "mp4" => "video/mp4",
        "mpeg" => "video/mpeg",
        _ => "application/octet-stream",
    }
}
