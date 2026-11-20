use lna_types::{ErrorCode, LnaError, LnaResult};
use std::net::{IpAddr, SocketAddr};
use url::{Host, Url};

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
    match url.host() {
        Some(Host::Domain(host)) => {
            if host.eq_ignore_ascii_case("localhost")
                || host.ends_with(".localhost")
                || host.ends_with(".local")
            {
                return Err(LnaError::new(
                    ErrorCode::UrlBlocked,
                    "Loopback and local hosts are blocked",
                ));
            }
        }
        Some(Host::Ipv4(v4)) => {
            if is_private_or_reserved(IpAddr::V4(v4)) {
                return Err(LnaError::new(
                    ErrorCode::UrlBlocked,
                    "Private or reserved IP addresses are blocked",
                ));
            }
        }
        Some(Host::Ipv6(v6)) => {
            if is_private_or_reserved(IpAddr::V6(v6)) {
                return Err(LnaError::new(
                    ErrorCode::UrlBlocked,
                    "Private or reserved IP addresses are blocked",
                ));
            }
        }
        None => {
            return Err(LnaError::new(ErrorCode::UrlBlocked, "URL missing host"));
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
                || v4.is_multicast()
                || v4.octets()[0] == 0
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0b1100_0000) == 64)
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_private_or_reserved(IpAddr::V4(mapped));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Fetch URL with manual redirect validation (max 5 redirects, 10 MB body limit, 15s timeout)
/// and pinned DNS resolution per hop to prevent DNS rebinding (B3).
pub async fn safe_fetch_url(raw_url: &str) -> LnaResult<(String, Vec<u8>)> {
    let mut current = validate_public_url(raw_url)?;
    for _ in 0..=5 {
        let raw_host = current
            .host_str()
            .ok_or_else(|| LnaError::new(ErrorCode::UrlBlocked, "Missing host"))?;
        let lookup_target = raw_host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let port = current.port_or_known_default().unwrap_or(80);
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((lookup_target.as_str(), port))
            .await
            .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?
            .collect();
        if addrs.is_empty() {
            return Err(LnaError::new(
                ErrorCode::FetchFailed,
                "DNS returned no addresses",
            ));
        }
        for addr in &addrs {
            if is_private_or_reserved(addr.ip()) {
                return Err(LnaError::new(
                    ErrorCode::UrlBlocked,
                    format!("Resolved IP {} is blocked", addr.ip()),
                ));
            }
        }

        // Pin the validated IP on this hop's reqwest::Client (B3 anti-DNS-rebinding)
        let mut client_builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15));
        if matches!(current.host(), Some(Host::Domain(_))) {
            client_builder = client_builder.resolve(&lookup_target, addrs[0]);
        }
        let client = client_builder
            .build()
            .map_err(|e| LnaError::internal(e.to_string()))?;

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

        const MAX_URL_BYTES: usize = 10 * 1024 * 1024;
        if let Some(cl) = resp.content_length() {
            if cl > MAX_URL_BYTES as u64 {
                return Err(LnaError::new(
                    ErrorCode::SourceTooLarge,
                    "URL response exceeded 10 MB limit",
                ));
            }
        }
        let mut resp = resp;
        let mut buf = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?
        {
            if buf.len() + chunk.len() > MAX_URL_BYTES {
                return Err(LnaError::new(
                    ErrorCode::SourceTooLarge,
                    "URL response exceeded 10 MB limit",
                ));
            }
            buf.extend_from_slice(&chunk);
        }
        return Ok((current.to_string(), buf));
    }

    Err(LnaError::new(
        ErrorCode::FetchFailed,
        "Too many redirects (max 5)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_ssrf_urls_and_private_ips() {
        for bad in [
            "http://localhost:8080/a",
            "http://sub.localhost/a",
            "http://127.0.0.1/admin",
            "http://0.0.0.0/",
            "http://10.0.0.1/internal",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data/",
            "file:///etc/passwd",
            "ftp://example.com/a",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:169.254.169.254]/",
        ] {
            assert!(
                validate_public_url(bad).is_err(),
                "Expected SSRF block for {bad}"
            );
        }
        assert!(validate_public_url("https://example.com/article").is_ok());
    }
}
