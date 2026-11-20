use lna_secrets::SecretStore;
use lna_storage::{db::map_sql_err, Database};
use lna_types::{new_ulid, now_ms, ErrorCode, LnaError, LnaResult};
use rusqlite::{params, OptionalExtension};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingModelRecord {
    pub id: String,
    pub provider: String,
    pub model_name: String,
    pub dimension: usize,
    pub query_prefix: String,
    pub passage_prefix: String,
    pub vec_table: String,
    pub endpoint_profile_id: Option<String>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingStatus {
    pub active_model: Option<EmbeddingModelRecord>,
    pub total_chunks: i64,
    pub embedded_chunks: i64,
    pub model_loaded: bool,
}

struct LoadedOnnx {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    last_used: Instant,
}

#[derive(Clone)]
pub struct EmbeddingService {
    db: Database,
    secrets: Arc<dyn SecretStore>,
    http: reqwest::Client,
    onnx: Arc<Mutex<Option<LoadedOnnx>>>,
}

impl EmbeddingService {
    pub fn new(db: Database, secrets: Arc<dyn SecretStore>) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            db,
            secrets,
            http,
            onnx: Arc::new(Mutex::new(None)),
        }
    }

    /// Unload ONNX Runtime if idle >= 120s (Section 8.4).
    pub fn evict_if_idle(&self) {
        if let Ok(mut g) = self.onnx.lock() {
            if let Some(ref loaded) = *g {
                if loaded.last_used.elapsed() >= Duration::from_secs(120) {
                    *g = None;
                    tracing::info!("Unloaded idle ONNX embedding session");
                }
            }
        }
    }

    pub fn active_model(&self) -> LnaResult<Option<EmbeddingModelRecord>> {
        self.db.read(|conn| {
            conn.query_row(
                "SELECT id, provider, model_name, dimension, query_prefix, passage_prefix,
                        vec_table, endpoint_profile_id, is_active
                 FROM embedding_models WHERE is_active = 1 LIMIT 1",
                [],
                |r| {
                    Ok(EmbeddingModelRecord {
                        id: r.get(0)?,
                        provider: r.get(1)?,
                        model_name: r.get(2)?,
                        dimension: r.get::<_, i64>(3)? as usize,
                        query_prefix: r.get(4)?,
                        passage_prefix: r.get(5)?,
                        vec_table: r.get(6)?,
                        endpoint_profile_id: r.get(7)?,
                        is_active: r.get::<_, i64>(8)? == 1,
                    })
                },
            )
            .optional()
            .map_err(map_sql_err)
        })
    }

    pub fn status(&self) -> LnaResult<EmbeddingStatus> {
        let active = self.active_model()?;
        let (total_chunks, embedded_chunks) = self.db.read(|conn| {
            let total: i64 = conn
                .query_row("SELECT COUNT(*) FROM source_chunks", [], |r| r.get(0))
                .map_err(map_sql_err)?;
            let embedded: i64 = if let Some(ref m) = active {
                conn.query_row(
                    "SELECT COUNT(*) FROM chunk_embedding_state WHERE embedding_model_id = ?1",
                    [&m.id],
                    |r| r.get(0),
                )
                .map_err(map_sql_err)?
            } else {
                0
            };
            Ok((total, embedded))
        })?;
        let model_loaded = self.onnx.lock().map(|g| g.is_some()).unwrap_or(false);
        Ok(EmbeddingStatus {
            active_model: active,
            total_chunks,
            embedded_chunks,
            model_loaded,
        })
    }

    pub fn configure(
        &self,
        provider: &str,
        model_name: &str,
        dimension: usize,
        query_prefix: &str,
        passage_prefix: &str,
        endpoint_profile_id: Option<&str>,
    ) -> LnaResult<EmbeddingModelRecord> {
        if provider != "local_onnx" && provider != "remote" {
            return Err(LnaError::invalid("provider must be local_onnx or remote"));
        }
        if dimension == 0 || dimension > 4096 {
            return Err(LnaError::invalid("dimension must be in 1..=4096"));
        }
        let id = new_ulid();
        // Safe server-generated table name (Section 6.3)
        let vec_table = format!("chunks_vec_{}", id.to_lowercase());
        let now = now_ms();

        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            tx.execute("UPDATE embedding_models SET is_active = 0 WHERE is_active = 1", [])
                .map_err(map_sql_err)?;
            tx.execute(
                "INSERT INTO embedding_models
                 (id, provider, model_name, dimension, query_prefix, passage_prefix, vec_table, endpoint_profile_id, is_active, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,1,?9)",
                params![
                    id,
                    provider,
                    model_name,
                    dimension as i64,
                    query_prefix,
                    passage_prefix,
                    vec_table,
                    endpoint_profile_id,
                    now
                ],
            )
            .map_err(map_sql_err)?;
            let create_vec_sql = format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS {vec_table} USING vec0(
                    chunk_id INTEGER PRIMARY KEY,
                    notebook_id TEXT PARTITION KEY,
                    embedding FLOAT[{dimension}] distance_metric=cosine
                );"
            );
            tx.execute_batch(&create_vec_sql).map_err(map_sql_err)?;
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })?;

        self.active_model()?
            .ok_or_else(|| LnaError::internal("Failed to load configured embedding model"))
    }

    pub async fn embed_query(&self, text: &str) -> LnaResult<Vec<f32>> {
        let model = self.active_model()?.ok_or_else(|| {
            LnaError::new(ErrorCode::CapabilityMissing, "No active embedding model")
        })?;
        let prefixed = format!("{}{}", model.query_prefix, text);
        let mut vecs = self.embed_batch(&model, &[prefixed]).await?;
        vecs.pop()
            .ok_or_else(|| LnaError::internal("Empty embedding result"))
    }

    pub async fn embed_passages(
        &self,
        model: &EmbeddingModelRecord,
        texts: &[String],
    ) -> LnaResult<Vec<Vec<f32>>> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| format!("{}{}", model.passage_prefix, t))
            .collect();
        self.embed_batch(model, &prefixed).await
    }

    async fn embed_batch(
        &self,
        model: &EmbeddingModelRecord,
        inputs: &[String],
    ) -> LnaResult<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        if model.provider == "remote" {
            return self.embed_remote(model, inputs).await;
        }
        let model_dir = self.db.paths().models_dir.join(&model.model_name);
        let onnx_arc = self.onnx.clone();
        let inputs_owned = inputs.to_vec();
        let dim = model.dimension;

        tokio::task::spawn_blocking(move || {
            Self::embed_local_onnx(onnx_arc, model_dir, &inputs_owned, dim)
        })
        .await
        .map_err(|e| LnaError::internal(format!("Embedding task join error: {e}")))?
    }

    fn embed_local_onnx(
        onnx_arc: Arc<Mutex<Option<LoadedOnnx>>>,
        model_dir: PathBuf,
        inputs: &[String],
        expected_dim: usize,
    ) -> LnaResult<Vec<Vec<f32>>> {
        let mut guard = onnx_arc
            .lock()
            .map_err(|_| LnaError::internal("ONNX lock poisoned"))?;
        if guard.is_none() {
            let onnx_path = model_dir.join("model.onnx");
            let tok_path = model_dir.join("tokenizer.json");
            if !onnx_path.exists() || !tok_path.exists() {
                return Err(LnaError::new(
                    ErrorCode::CapabilityMissing,
                    format!(
                        "Local ONNX model files missing in {}. Place model.onnx and tokenizer.json there or configure a remote embedding provider.",
                        model_dir.display()
                    ),
                ));
            }
            let session = ort::session::Session::builder()
                .map_err(|e| LnaError::internal(e.to_string()))?
                .with_intra_threads(1)
                .map_err(|e| LnaError::internal(e.to_string()))?
                .commit_from_file(&onnx_path)
                .map_err(|e| LnaError::internal(e.to_string()))?;
            let tokenizer = tokenizers::Tokenizer::from_file(&tok_path)
                .map_err(|e| LnaError::internal(e.to_string()))?;
            *guard = Some(LoadedOnnx {
                session,
                tokenizer,
                last_used: Instant::now(),
            });
        }

        let loaded = guard.as_mut().unwrap();
        loaded.last_used = Instant::now();

        let mut results = Vec::with_capacity(inputs.len());
        for text in inputs {
            let enc = loaded
                .tokenizer
                .encode(text.as_str(), true)
                .map_err(|e| LnaError::internal(e.to_string()))?;
            let ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
            let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&x| x as i64).collect();
            let seq_len = ids.len();

            let input_ids = ort::value::Tensor::from_array(([1, seq_len], ids))
                .map_err(|e| LnaError::internal(e.to_string()))?;
            let attn_mask = ort::value::Tensor::from_array(([1, seq_len], mask))
                .map_err(|e| LnaError::internal(e.to_string()))?;

            let outputs = loaded
                .session
                .run(ort::inputs!["input_ids" => input_ids, "attention_mask" => attn_mask])
                .map_err(|e| LnaError::internal(e.to_string()))?;

            let (_shape, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| LnaError::internal(e.to_string()))?;

            let mut pooled = vec![0.0f32; expected_dim.min(data.len())];
            let tokens = (data.len() / expected_dim).max(1);
            for t in 0..tokens {
                for d in 0..pooled.len() {
                    pooled[d] += data[t * expected_dim + d];
                }
            }
            for v in &mut pooled {
                *v /= tokens as f32;
            }
            l2_normalize(&mut pooled);
            results.push(pooled);
        }
        Ok(results)
    }

    async fn embed_remote(
        &self,
        model: &EmbeddingModelRecord,
        inputs: &[String],
    ) -> LnaResult<Vec<Vec<f32>>> {
        let profile_id = model.endpoint_profile_id.as_deref().ok_or_else(|| {
            LnaError::new(
                ErrorCode::ModelNotConfigured,
                "Remote embedding model requires endpoint_profile_id",
            )
        })?;
        let profile = lna_storage::get_profile(&self.db, profile_id)?;
        let api_key = match profile.keychain_ref {
            Some(ref kref) => self.secrets.get(kref)?,
            None => None,
        };

        let url = format!("{}/embeddings", profile.base_url.trim_end_matches('/'));
        let mut req = self.http.post(url).json(&json!({
            "model": model.model_name,
            "input": inputs
        }));
        if let Some(ref key) = api_key {
            let s = key.expose_secret();
            if !s.is_empty() {
                req = req.bearer_auth(s);
            }
        }
        let resp = req
            .send()
            .await
            .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(LnaError::new(
                ErrorCode::FetchFailed,
                format!("Remote embedding HTTP {}: {}", status.as_u16(), text),
            ));
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| LnaError::new(ErrorCode::OutputInvalid, e.to_string()))?;
        let arr = v.get("data").and_then(|d| d.as_array()).ok_or_else(|| {
            LnaError::new(ErrorCode::OutputInvalid, "Missing embedding data array")
        })?;
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let emb = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| {
                    LnaError::new(ErrorCode::OutputInvalid, "Missing embedding vector")
                })?;
            let mut vec: Vec<f32> = emb
                .iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect();
            l2_normalize(&mut vec);
            out.push(vec);
        }
        Ok(out)
    }

    pub fn store_chunk_embeddings(
        &self,
        model: &EmbeddingModelRecord,
        notebook_id: &str,
        items: &[(i64, Vec<f32>)],
    ) -> LnaResult<()> {
        if items.is_empty() {
            return Ok(());
        }
        let tbl = &model.vec_table;
        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            let insert_vec = format!(
                "INSERT OR REPLACE INTO {tbl} (chunk_id, notebook_id, embedding) VALUES (?1, ?2, ?3)"
            );
            for (chunk_id, vec) in items {
                let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
                tx.execute(&insert_vec, params![chunk_id, notebook_id, bytes])
                    .map_err(map_sql_err)?;
                tx.execute(
                    "INSERT OR IGNORE INTO chunk_embedding_state (chunk_id, embedding_model_id) VALUES (?1, ?2)",
                    params![chunk_id, model.id],
                )
                .map_err(map_sql_err)?;
            }
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })
    }
}

pub fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for x in v {
            *x /= norm;
        }
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..len {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom > 1e-12 {
        dot / denom
    } else {
        0.0
    }
}
