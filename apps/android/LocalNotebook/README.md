# Local Notebook Agent — Android Client

Native Android client built with **Kotlin + Jetpack Compose + Material 3**, connected directly to the shared Rust `lna-ffi` core via JNI (`crates/lna-ffi/src/android_jni.rs`).

## Features
- **Responsive Phone & Tablet Layout**: 3-tab bottom navigation on phones, 3-pane split workspace on tablets (`>= 840dp`), with animated fullscreen transitions.
- **Sources & Native PDF Viewer**: Import local files via Android Storage Access Framework (SAF), web URLs, or pasted text; collapsible Source Guide with horizontal flow-wrap topic pills, AI Knowledge Graph recommendation bar, and native `PdfRenderer` page viewer.
- **Chat & Smart Citations**: Readable Material 3 typography (`15sp` default, uniform SemiBold), collapsible Thoughts, and Wikipedia-style Reference Card popovers that automatically open the native PDF viewer at the exact page when an excerpt is unclear or figure-only.
- **Studio & Knowledge Graph**: 4×2 8-tile Studio grid (Audio Overview, Slides, Mind Map, Report, Flashcards, Quiz, Infographic, Knowledge Graph), Prompt & Sources popover, interactive collapsible Mind Map / Knowledge Graph tree with tap-to-Quiz / Expand Mind Map / Ask in Chat linkage, and clean error panel with collapsible diagnostic log.
- **Unified Settings & Localization**: 4-tab Settings bottom sheet (Appearance, General, AI Models, Semantic Search) with `zh-CN` / `en-US` localization, automatic AI translation of notebook summaries & source guides (preserving chat history), and Android Keystore (`EncryptedSharedPreferences`) secret storage.

## Build Steps
1. **Compile Rust core for Android** (requires Android NDK & `cargo-ndk`):
   ```bash
   rustup target add aarch64-linux-android x86_64-linux-android
   cargo install cargo-ndk
   export ANDROID_NDK_HOME=/path/to/android-ndk
   bash apps/android/LocalNotebook/build-android-rust.sh
   ```
2. **Open in Android Studio** (`apps/android/LocalNotebook`) or run Gradle:
   ```bash
   cd apps/android/LocalNotebook
   gradle assembleDebug
   ```
