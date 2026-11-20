//! Downloads a local embedding model (ONNX + tokenizer) with checksum verification (B8 / B-15).
//!
//! Files are streamed to `<name>.part`, hashed on the fly, and only renamed into place when the
//! SHA-256 matches, so a half-downloaded or tampered file is never loaded.

use crate::EmbeddingService;
use futures_util::StreamExt;
use lna_types::{ErrorCode, LnaError, LnaResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFile {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Directory name below the models folder; also the `model_name` of the embedding record.
    pub id: String,
    pub display_name: String,
    pub dimension: usize,
    pub query_prefix: String,
    pub passage_prefix: String,
    pub files: Vec<ModelFile>,
}

impl ModelSpec {
    /// `intfloat/multilingual-e5-small` (MIT licence), pinned to a fixed revision.
    /// 384 dimensions, handles Chinese and English, about 470 MB for the fp32 ONNX export.
    pub fn default_multilingual() -> Self {
        let rev = "614241f622f53c4eeff9890bdc4f31cfecc418b3";
        let base = format!("https://huggingface.co/intfloat/multilingual-e5-small/resolve/{rev}");
        ModelSpec {
            id: "multilingual-e5-small".into(),
            display_name: "multilingual-e5-small（中英双语，约 470 MB）".into(),
            dimension: 384,
            query_prefix: "query: ".into(),
            passage_prefix: "passage: ".into(),
            files: vec![
                ModelFile {
                    name: "tokenizer.json".into(),
                    url: format!("{base}/tokenizer.json"),
                    sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39"
                        .into(),
                    size: 17_082_730,
                },
                ModelFile {
                    name: "model.onnx".into(),
                    url: format!("{base}/onnx/model.onnx"),
                    sha256: "ca456c06b3a9505ddfd9131408916dd79290368331e7d76bb621f1cba6bc8665"
                        .into(),
                    size: 470_268_510,
                },
            ],
        }
    }

    pub fn validate(&self) -> LnaResult<()> {
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            || self.id.starts_with('.')
        {
            return Err(LnaError::invalid(
                "model id may only contain letters, digits, - _ .",
            ));
        }
        if self.dimension == 0 || self.dimension > 4096 {
            return Err(LnaError::invalid("dimension must be in 1..=4096"));
        }
        let names: Vec<&str> = self.files.iter().map(|f| f.name.as_str()).collect();
        if !names.contains(&"model.onnx") || !names.contains(&"tokenizer.json") {
            return Err(LnaError::invalid(
                "a model needs model.onnx and tokenizer.json",
            ));
        }
        for f in &self.files {
            if f.name.contains('/')
                || f.name.contains('\\')
                || f.name.contains("..")
                || f.name.is_empty()
            {
                return Err(LnaError::invalid(format!("unsafe file name: {}", f.name)));
            }
            if f.sha256.len() != 64 || !f.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(LnaError::invalid(format!(
                    "{}: sha256 must be 64 hex characters",
                    f.name
                )));
            }
            let url = url::Url::parse(&f.url)
                .map_err(|e| LnaError::invalid(format!("{}: {e}", f.name)))?;
            let loopback = matches!(
                url.host_str(),
                Some("127.0.0.1") | Some("localhost") | Some("[::1]")
            );
            if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
                return Err(LnaError::invalid(format!(
                    "{}: model files must be fetched over https",
                    f.name
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub file: String,
    pub file_index: usize,
    pub file_count: usize,
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// Bytes of all files finished so far plus the current file's progress.
    pub overall_done: u64,
    pub overall_total: u64,
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

impl EmbeddingService {
    pub fn model_dir(&self, spec_id: &str) -> PathBuf {
        self.db.paths().models_dir.join(spec_id)
    }

    /// True when every file of the spec is present (sizes are checked; hashes were verified on download).
    pub fn model_files_present(&self, spec: &ModelSpec) -> bool {
        let dir = self.model_dir(&spec.id);
        spec.files.iter().all(|f| {
            std::fs::metadata(dir.join(&f.name))
                .map(|m| m.len() == f.size)
                .unwrap_or(false)
        })
    }

    pub async fn download_model(
        &self,
        spec: &ModelSpec,
        cancel: CancellationToken,
        mut on_progress: impl FnMut(DownloadProgress),
    ) -> LnaResult<PathBuf> {
        spec.validate()?;
        let dir = self.model_dir(&spec.id);
        std::fs::create_dir_all(&dir).map_err(|e| LnaError::internal(e.to_string()))?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| LnaError::internal(e.to_string()))?;
        let overall_total: u64 = spec.files.iter().map(|f| f.size).sum();
        let mut overall_done = 0u64;

        for (idx, file) in spec.files.iter().enumerate() {
            let target = dir.join(&file.name);
            let part = dir.join(format!("{}.part", file.name));
            let report = |done: u64, overall: u64, cb: &mut dyn FnMut(DownloadProgress)| {
                cb(DownloadProgress {
                    file: file.name.clone(),
                    file_index: idx,
                    file_count: spec.files.len(),
                    done_bytes: done,
                    total_bytes: file.size,
                    overall_done: overall,
                    overall_total,
                })
            };

            // Already there and intact: nothing to do.
            if target.exists() {
                let t = target.clone();
                let sum = tokio::task::spawn_blocking(move || sha256_file(&t))
                    .await
                    .map_err(|e| LnaError::internal(e.to_string()))?
                    .map_err(|e| LnaError::internal(e.to_string()))?;
                if sum.eq_ignore_ascii_case(&file.sha256) {
                    overall_done += file.size;
                    report(file.size, overall_done, &mut on_progress);
                    continue;
                }
                let _ = std::fs::remove_file(&target);
            }

            let resp = tokio::select! {
                _ = cancel.cancelled() => return Err(LnaError::new(ErrorCode::Cancelled, "Download cancelled")),
                r = http.get(&file.url).send() => r.map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?,
            };
            if !resp.status().is_success() {
                return Err(LnaError::new(
                    ErrorCode::FetchFailed,
                    format!("{}: HTTP {}", file.name, resp.status().as_u16()),
                ));
            }
            let mut out = tokio::fs::File::create(&part)
                .await
                .map_err(|e| LnaError::internal(e.to_string()))?;
            let mut hasher = Sha256::new();
            let mut done = 0u64;
            let mut last_report = std::time::Instant::now();
            let mut stream = resp.bytes_stream();
            loop {
                let next = tokio::select! {
                    _ = cancel.cancelled() => {
                        drop(out);
                        let _ = std::fs::remove_file(&part);
                        return Err(LnaError::new(ErrorCode::Cancelled, "Download cancelled"));
                    }
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {
                        return Err(LnaError::new(ErrorCode::FetchFailed, format!("{}: no data for 60 s", file.name)));
                    }
                    n = stream.next() => n,
                };
                match next {
                    Some(Ok(bytes)) => {
                        hasher.update(&bytes);
                        out.write_all(&bytes)
                            .await
                            .map_err(|e| LnaError::internal(e.to_string()))?;
                        done += bytes.len() as u64;
                        if last_report.elapsed() >= Duration::from_millis(250) {
                            last_report = std::time::Instant::now();
                            report(done, overall_done + done, &mut on_progress);
                        }
                    }
                    Some(Err(e)) => {
                        let _ = std::fs::remove_file(&part);
                        return Err(LnaError::new(ErrorCode::FetchFailed, e.to_string()));
                    }
                    None => break,
                }
            }
            out.flush()
                .await
                .map_err(|e| LnaError::internal(e.to_string()))?;
            drop(out);
            let got = hex::encode(hasher.finalize());
            if !got.eq_ignore_ascii_case(&file.sha256) || done != file.size {
                let _ = std::fs::remove_file(&part);
                return Err(LnaError::new(
                    ErrorCode::FetchFailed,
                    format!(
                        "{}: checksum mismatch (the download was corrupted or altered)",
                        file.name
                    ),
                ));
            }
            std::fs::rename(&part, &target).map_err(|e| LnaError::internal(e.to_string()))?;
            overall_done += file.size;
            report(file.size, overall_done, &mut on_progress);
        }
        Ok(dir)
    }
}
