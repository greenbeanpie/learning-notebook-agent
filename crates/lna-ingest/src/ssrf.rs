use lna_types::{ErrorCode, LnaError, LnaResult};
use std::net::IpAddr;
use url::Url;

pub fn validate_public_url(raw: &str) -> LnaResult<Url> {
    let url = Url::parse(raw).map_err(|e| LnaError::invalid(format!("Invalid URL: {e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(LnaError::new(
                ErrorCode::UrlBlocked,
                "Only http and https URLs are allowed",
            ))
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| LnaError::new(ErrorCode::UrlBlocked, "URL missing host"))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return Err(LnaError::new(
            ErrorCode::UrlBlocked,
            "Loopback and local hosts are blocked",
        ));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_or_reserved(ip) {
            return Err(LnaError::new(
                ErrorCode::UrlBlocked,
                "Private or reserved IP addresses are blocked",
            ));
        }
    }
    Ok(url)
}

pub fn is_private_or_reserved(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0b1100_0000) == 64
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Fetch URL with manual redirect validation (max 5 redirects, 10 MB body limit, 15s timeout).
pub async fn safe_fetch_url(raw_url: &str) -> LnaResult<(String, Vec<u8>)> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| LnaError::internal(e.to_string()))?;

    let mut current = validate_public_url(raw_url)?;
    for _ in 0..=5 {
        let host = current
            .host_str()
            .ok_or_else(|| LnaError::new(ErrorCode::UrlBlocked, "Missing host"))?;
        let port = current.port_or_known_default().unwrap_or(80);
        let addrs = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;
        for addr in addrs {
            if is_private_or_reserved(addr.ip()) {
                return Err(LnaError::new(
                    ErrorCode::UrlBlocked,
                    format!("Resolved IP {} is blocked", addr.ip()),
                ));
            }
        }

        let resp = client
            .get(current.clone())
            .header("User-Agent", "LocalNotebookAgent/0.1")
            .send()
            .await
            .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;

        if resp.status().is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    LnaError::new(ErrorCode::FetchFailed, "Redirect missing Location")
                })?;
            let next = current
                .join(loc)
                .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;
            current = validate_public_url(next.as_str())?;
            continue;
        }

        if !resp.status().is_success() {
            return Err(LnaError::new(
                ErrorCode::FetchFailed,
                format!("HTTP {}", resp.status().as_u16()),
            ));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;
        if bytes.len() > 10 * 1024 * 1024 {
            return Err(LnaError::new(
                ErrorCode::SourceTooLarge,
                "URL response exceeded 10 MB limit",
            ));
        }
        return Ok((current.to_string(), bytes.to_vec()));
    }

    Err(LnaError::new(
        ErrorCode::FetchFailed,
        "Too many redirects (max 5)",
    ))
}
