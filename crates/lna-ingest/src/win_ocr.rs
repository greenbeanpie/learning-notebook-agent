use std::path::Path;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs Windows built-in WinRT OCR (`Windows.Media.Ocr.OcrEngine`) across one or more image files
/// in a single headless PowerShell invocation. Returns recognized UTF-8 text per input file.
pub fn windows_ocr_image_files(paths: &[&Path]) -> Vec<Option<String>> {
    #[cfg(target_os = "macos")]
    {
        macos_ocr_image_files(paths)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        vec![None; paths.len()]
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if paths.is_empty() {
            return Vec::new();
        }
        let temp_id = format!("{}_{}", std::process::id(), lna_types::now_ms());
        let list_path = std::env::temp_dir().join(format!("lna_ocr_in_{temp_id}.txt"));
        let out_path = std::env::temp_dir().join(format!("lna_ocr_out_{temp_id}.json"));
        let lines: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        if std::fs::write(&list_path, lines.join("\n")).is_err() {
            return vec![None; paths.len()];
        }

        let ps_script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapPixelFormat, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapAlphaMode, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrEngine, Windows.Media.Ocr, ContentType=WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType=WindowsRuntime]

$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await-WinRt($WinRtTask, $ResultType) {
    $asTask = $asTaskGeneric.MakeGenericMethod($ResultType)
    $netTask = $asTask.Invoke($null, @($WinRtTask))
    $netTask.Wait(-1) | Out-Null
    $netTask.Result
}

$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $engine) {
    $langs = [Windows.Media.Ocr.OcrEngine]::AvailableRecognizerLanguages
    if ($langs.Count -gt 0) {
        $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage($langs[0])
    }
}
$results = @()
$files = [System.IO.File]::ReadAllLines($env:LNA_OCR_LIST, [System.Text.UTF8Encoding]::new($false))
foreach ($p in $files) {
    if ([string]::IsNullOrWhiteSpace($p) -or -not (Test-Path -LiteralPath $p) -or $null -eq $engine) {
        $results += ''
        continue
    }
    $fs = $null
    $ras = $null
    $sb = $null
    try {
        $fs = [System.IO.File]::OpenRead($p)
        $ras = [System.IO.WindowsRuntimeStreamExtensions]::AsRandomAccessStream($fs)
        $dec = Await-WinRt ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($ras)) ([Windows.Graphics.Imaging.BitmapDecoder])
        $sb = Await-WinRt ($dec.GetSoftwareBitmapAsync([Windows.Graphics.Imaging.BitmapPixelFormat]::Bgra8, [Windows.Graphics.Imaging.BitmapAlphaMode]::Premultiplied)) ([Windows.Graphics.Imaging.SoftwareBitmap])
        $res = Await-WinRt ($engine.RecognizeAsync($sb)) ([Windows.Media.Ocr.OcrResult])
        $txt = ($res.Lines | ForEach-Object { $_.Text }) -join "`n"
        $results += $txt
    } catch {
        $results += ''
    } finally {
        if ($null -ne $sb) { $sb.Dispose() }
        if ($null -ne $ras) { $ras.Dispose() }
        if ($null -ne $fs) { $fs.Dispose() }
    }
}
$json = ConvertTo-Json -InputObject $results -Compress
[System.IO.File]::WriteAllText($env:LNA_OCR_OUT, $json, [System.Text.UTF8Encoding]::new($false))
"#;

        let status = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                ps_script,
            ])
            .env("LNA_OCR_LIST", &list_path)
            .env("LNA_OCR_OUT", &out_path)
            .creation_flags(CREATE_NO_WINDOW)
            .output();

        let _ = std::fs::remove_file(&list_path);
        let mut out = vec![None; paths.len()];
        if let Ok(output) = status {
            if output.status.success() {
                if let Ok(json_str) = std::fs::read_to_string(&out_path) {
                    if let Ok(arr) = serde_json::from_str::<Vec<String>>(&json_str) {
                        for (i, s) in arr.into_iter().enumerate() {
                            if i < out.len() {
                                let t = s.trim().to_string();
                                if !t.is_empty() {
                                    out[i] = Some(t);
                                }
                            }
                        }
                    } else if let Ok(single) = serde_json::from_str::<String>(&json_str) {
                        if !out.is_empty() {
                            let t = single.trim().to_string();
                            if !t.is_empty() {
                                out[0] = Some(t);
                            }
                        }
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&out_path);
        out
    }
}

pub fn windows_ocr_image_bytes(image_bytes: &[u8], ext: &str) -> Option<String> {
    let clean_ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let suffix = if clean_ext.is_empty() {
        "png"
    } else {
        clean_ext.as_str()
    };
    let tmp = std::env::temp_dir().join(format!(
        "lna_ocr_{}_{}.{}",
        std::process::id(),
        lna_types::now_ms(),
        suffix
    ));
    if std::fs::write(&tmp, image_bytes).is_err() {
        return None;
    }
    let res = windows_ocr_image_files(&[tmp.as_path()])
        .into_iter()
        .next()
        .flatten();
    let _ = std::fs::remove_file(&tmp);
    res
}

/// Uses Windows Imaging Component (WinRT BitmapDecoder/BitmapEncoder) to transcode exotic formats
/// (`avif`, `heic`, `heif`, `jp2`) into PNG bytes when OS codecs are present.
pub fn windows_wic_to_png(input_path: &Path) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        macos_sips_to_png(input_path)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = input_path;
        None
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out_path = std::env::temp_dir().join(format!(
            "lna_wic_{}_{}.png",
            std::process::id(),
            lna_types::now_ms()
        ));
        let ps_script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapEncoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Storage.Streams.InMemoryRandomAccessStream, Windows.Storage.Streams, ContentType=WindowsRuntime]

$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
$asTaskAction = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncAction' })[0]
function Await-WinRt($WinRtTask, $ResultType) {
    $asTask = $asTaskGeneric.MakeGenericMethod($ResultType)
    $netTask = $asTask.Invoke($null, @($WinRtTask))
    $netTask.Wait(-1) | Out-Null
    $netTask.Result
}
function Await-WinRtAction($WinRtTask) {
    $netTask = $asTaskAction.Invoke($null, @($WinRtTask))
    $netTask.Wait(-1) | Out-Null
}

$fsIn = [System.IO.File]::OpenRead($env:LNA_WIC_IN)
$rasIn = [System.IO.WindowsRuntimeStreamExtensions]::AsRandomAccessStream($fsIn)
$dec = Await-WinRt ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($rasIn)) ([Windows.Graphics.Imaging.BitmapDecoder])
$sb = Await-WinRt ($dec.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])

$mem = New-Object Windows.Storage.Streams.InMemoryRandomAccessStream
$enc = Await-WinRt ([Windows.Graphics.Imaging.BitmapEncoder]::CreateAsync([Windows.Graphics.Imaging.BitmapEncoder]::PngEncoderId, $mem)) ([Windows.Graphics.Imaging.BitmapEncoder])
$enc.SetSoftwareBitmap($sb)
Await-WinRtAction ($enc.FlushAsync())

$memStream = [System.IO.WindowsRuntimeStreamExtensions]::AsStreamForRead($mem.GetInputStreamAt(0))
$fsOut = [System.IO.File]::Create($env:LNA_WIC_OUT)
$memStream.CopyTo($fsOut)
$fsOut.Dispose()
$memStream.Dispose()
$mem.Dispose()
$sb.Dispose()
$rasIn.Dispose()
$fsIn.Dispose()
"#;
        let res = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                ps_script,
            ])
            .env("LNA_WIC_IN", input_path)
            .env("LNA_WIC_OUT", &out_path)
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let bytes = if matches!(res, Ok(o) if o.status.success()) {
            std::fs::read(&out_path).ok()
        } else {
            None
        };
        let _ = std::fs::remove_file(&out_path);
        bytes
    }
}

/// macOS Vision text recognition, driven through a JavaScript-for-Automation script (`osascript`),
/// so no extra native bindings are needed. One run recognizes all files; a failed file is `None`.
#[cfg(target_os = "macos")]
fn macos_ocr_image_files(paths: &[&Path]) -> Vec<Option<String>> {
    if paths.is_empty() {
        return Vec::new();
    }
    let temp_id = format!("{}_{}", std::process::id(), lna_types::now_ms());
    let script_path = std::env::temp_dir().join(format!("lna_ocr_{temp_id}.js"));
    let list_path = std::env::temp_dir().join(format!("lna_ocr_in_{temp_id}.txt"));
    let out_path = std::env::temp_dir().join(format!("lna_ocr_out_{temp_id}.json"));
    let lines: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let cleanup = || {
        let _ = std::fs::remove_file(&script_path);
        let _ = std::fs::remove_file(&list_path);
        let _ = std::fs::remove_file(&out_path);
    };
    if std::fs::write(&script_path, include_str!("macos_ocr.js")).is_err()
        || std::fs::write(&list_path, lines.join("\n")).is_err()
    {
        cleanup();
        return vec![None; paths.len()];
    }
    let ran = std::process::Command::new("osascript")
        .args(["-l", "JavaScript"])
        .arg(&script_path)
        .arg(&list_path)
        .arg(&out_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let mut out = vec![None; paths.len()];
    if matches!(ran, Ok(st) if st.success()) {
        if let Some(arr) = std::fs::read_to_string(&out_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<Option<String>>>(&t).ok())
        {
            for (slot, v) in out.iter_mut().zip(arr) {
                *slot = v.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
            }
        }
    }
    cleanup();
    out
}

/// Decodes HEIC / AVIF / JP2 (and anything else ImageIO reads) to PNG with the system `sips`.
#[cfg(target_os = "macos")]
fn macos_sips_to_png(input_path: &Path) -> Option<Vec<u8>> {
    let out = std::env::temp_dir().join(format!(
        "lna_sips_{}_{}.png",
        std::process::id(),
        lna_types::now_ms()
    ));
    let ok = std::process::Command::new("sips")
        .args(["-s", "format", "png"])
        .arg(input_path)
        .arg("--out")
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let bytes = if ok { std::fs::read(&out).ok() } else { None };
    let _ = std::fs::remove_file(&out);
    bytes
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    /// Runs only with `LNA_TEST_OCR_IMAGE=/path/to/image.png` (needs a picture with text).
    #[test]
    fn vision_ocr_reads_text_from_an_image() {
        let Some(img) = std::env::var_os("LNA_TEST_OCR_IMAGE") else {
            return;
        };
        let img = std::path::PathBuf::from(img);
        let res = windows_ocr_image_files(&[img.as_path(), Path::new("/nonexistent.png")]);
        assert_eq!(res.len(), 2);
        let text = res[0].clone().expect("text recognized");
        assert!(
            text.to_lowercase().contains("linear algebra"),
            "got: {text}"
        );
        assert!(res[1].is_none());
    }

    #[test]
    fn sips_transcodes_png_to_png_and_rejects_garbage() {
        let tmp =
            std::env::temp_dir().join(format!("lna_sips_garbage_{}.heic", std::process::id()));
        std::fs::write(&tmp, b"not an image").unwrap();
        assert!(windows_wic_to_png(&tmp).is_none());
        let _ = std::fs::remove_file(&tmp);
    }
}
