//! Helpers shared by unit tests.

/// Minimal valid 8-bit mono PCM WAV, `ms` milliseconds of silence.
pub fn wav_bytes(ms: u32) -> Vec<u8> {
    let rate = 8000u32;
    let data_len = rate * ms / 1000;
    let mut v = Vec::new();
    v.extend(b"RIFF");
    v.extend((36 + data_len).to_le_bytes());
    v.extend(b"WAVEfmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes()); // PCM
    v.extend(1u16.to_le_bytes()); // mono
    v.extend(rate.to_le_bytes());
    v.extend(rate.to_le_bytes()); // byte rate
    v.extend(1u16.to_le_bytes()); // block align
    v.extend(8u16.to_le_bytes()); // bits
    v.extend(b"data");
    v.extend(data_len.to_le_bytes());
    v.extend(std::iter::repeat_n(128u8, data_len as usize));
    v
}

/// Serve one canned HTTP response on localhost. Returns the base URL and a
/// handle that yields the raw request text once the request was answered.
pub fn serve_once(
    status_line: &'static str,
    body: impl Into<Vec<u8>>,
) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let body: Vec<u8> = body.into();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
        }
        let head = format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
            body.len()
        );
        // The client may hang up early (e.g. in failure tests).
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body);
        String::from_utf8_lossy(&request).into_owned()
    });
    (url, handle)
}
