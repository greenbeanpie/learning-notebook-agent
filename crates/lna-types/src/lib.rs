pub mod api;
pub mod domain;
pub mod error;

pub use api::*;
pub use domain::*;
pub use error::*;

pub fn new_ulid() -> String {
    ulid::Ulid::new().to_string()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
