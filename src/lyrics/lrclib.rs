//! Client for the LRCLIB API (https://lrclib.net).

use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

pub const BASE_URL: &str = "https://lrclib.net";
const TIMEOUT: Duration = Duration::from_secs(5);
const USER_AGENT: &str = concat!(
    "synq/",
    env!("CARGO_PKG_VERSION"),
    " (terminal music player)"
);

#[derive(Debug, Error)]
pub enum LrclibError {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("LRCLIB answered with status {0}")]
    Status(u16),
    #[error("unexpected LRCLIB response: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct Query<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub album: &'a str,
    pub duration_secs: u64,
}

/// What LRCLIB knows about a track. Both texts can be present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub synced: Option<String>,
    pub plain: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

pub fn client() -> Result<reqwest::blocking::Client, LrclibError> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(TIMEOUT)
        .build()?)
}

/// `Ok(None)` means LRCLIB has no lyrics for the track (a 404, or a track
/// marked instrumental); errors mean "could not ask" and must not be cached.
pub fn fetch(
    client: &reqwest::blocking::Client,
    base_url: &str,
    q: &Query<'_>,
) -> Result<Option<Fetched>, LrclibError> {
    let mut params = vec![
        ("artist_name", q.artist.to_owned()),
        ("track_name", q.title.to_owned()),
    ];
    if !q.album.is_empty() {
        params.push(("album_name", q.album.to_owned()));
    }
    if q.duration_secs > 0 {
        params.push(("duration", q.duration_secs.to_string()));
    }
    let resp = client
        .get(format!("{base_url}/api/get"))
        .query(&params)
        .send()?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(LrclibError::Status(status.as_u16()));
    }
    parse_response(&resp.text()?)
}

pub fn parse_response(body: &str) -> Result<Option<Fetched>, LrclibError> {
    let r: Response = serde_json::from_str(body)?;
    let non_empty = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
    let (synced, plain) = (non_empty(r.synced_lyrics), non_empty(r.plain_lyrics));
    Ok(if synced.is_none() && plain.is_none() {
        None
    } else {
        Some(Fetched { synced, plain })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::serve_once;
    use std::net::TcpListener;

    fn query() -> Query<'static> {
        Query {
            artist: "AC/DC",
            title: "Back in Black",
            album: "Back in Black",
            duration_secs: 255,
        }
    }

    #[test]
    fn parses_synced_and_plain() {
        let f = parse_response(
            r#"{"id":1,"instrumental":false,"syncedLyrics":"[00:01.00]a","plainLyrics":"a"}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(f.synced.as_deref(), Some("[00:01.00]a"));
        assert_eq!(f.plain.as_deref(), Some("a"));
    }

    #[test]
    fn null_or_blank_lyrics_mean_nothing_found() {
        for body in [
            r#"{"syncedLyrics":null,"plainLyrics":null}"#,
            r#"{"syncedLyrics":"","plainLyrics":"  "}"#,
            r#"{"instrumental":true}"#,
        ] {
            assert_eq!(parse_response(body).unwrap(), None, "{body}");
        }
    }

    #[test]
    fn plain_only_is_kept() {
        let f = parse_response(r#"{"syncedLyrics":null,"plainLyrics":"words"}"#)
            .unwrap()
            .unwrap();
        assert_eq!((f.synced, f.plain.as_deref()), (None, Some("words")));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse_response("<html>").is_err());
    }

    #[test]
    fn sends_user_agent_and_encoded_parameters() {
        let (url, server) = serve_once("200 OK", r#"{"plainLyrics":"la la"}"#);
        let found = fetch(&client().unwrap(), &url, &query()).unwrap().unwrap();
        assert_eq!(found.plain.as_deref(), Some("la la"));

        let request = server.join().unwrap();
        let first_line = request.lines().next().unwrap();
        assert!(first_line.starts_with("GET /api/get?"), "{first_line}");
        assert!(first_line.contains("artist_name=AC%2FDC"), "{first_line}");
        assert!(
            first_line.contains("track_name=Back+in+Black")
                || first_line.contains("track_name=Back%20in%20Black")
        );
        assert!(first_line.contains("album_name="));
        assert!(first_line.contains("duration=255"));
        let ua = request
            .lines()
            .find(|l| l.to_lowercase().starts_with("user-agent:"))
            .unwrap();
        assert!(ua.contains("synq/"), "{ua}");
    }

    #[test]
    fn omits_unknown_album_and_duration() {
        let (url, server) = serve_once("404 Not Found", "{}");
        let q = Query {
            album: "",
            duration_secs: 0,
            ..query()
        };
        assert_eq!(fetch(&client().unwrap(), &url, &q).unwrap(), None);
        let request = server.join().unwrap();
        let first_line = request.lines().next().unwrap();
        assert!(!first_line.contains("album_name") && !first_line.contains("duration"));
    }

    #[test]
    fn not_found_is_none_but_server_errors_are_errors() {
        let (url, _s) = serve_once("404 Not Found", r#"{"statusCode":404}"#);
        assert_eq!(fetch(&client().unwrap(), &url, &query()).unwrap(), None);

        let (url, _s) = serve_once("500 Internal Server Error", "oops");
        assert!(matches!(
            fetch(&client().unwrap(), &url, &query()),
            Err(LrclibError::Status(500))
        ));
    }

    #[test]
    fn unreachable_server_is_an_error_not_a_hang() {
        // Nothing listens on this port (bound then dropped).
        let url = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let started = std::time::Instant::now();
        assert!(matches!(
            fetch(&client().unwrap(), &url, &query()),
            Err(LrclibError::Http(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
