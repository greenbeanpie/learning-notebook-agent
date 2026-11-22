use std::path::Path;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs Windows built-in WinRT OCR (`Windows.Media.Ocr.OcrEngine`) across one or more image files
/// in a single headless PowerShell invocation. Returns recognized UTF-8 text per input file.
pub fn windows_ocr_image_files(paths: &[&Path]) -> Vec<Option<String>> {
    #[cfg(not(windows))]
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
    #[cfg(not(windows))]
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
