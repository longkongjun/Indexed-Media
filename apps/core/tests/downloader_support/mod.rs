#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

pub struct ScriptedResponse {
    bytes: Vec<u8>,
    delay: Duration,
}

impl ScriptedResponse {
    pub fn new(status: &str, headers: &[(&str, &str)], body: impl AsRef<[u8]>) -> Self {
        let body = body.as_ref();
        let has_content_length = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        let mut bytes = format!("HTTP/1.1 {status}\r\nConnection: close\r\n").into_bytes();
        if !has_content_length {
            bytes.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
        }
        for (name, value) in headers {
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(b": ");
            bytes.extend_from_slice(value.as_bytes());
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(body);
        Self {
            bytes,
            delay: Duration::ZERO,
        }
    }

    pub fn empty_connection() -> Self {
        Self {
            bytes: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

pub struct ScriptedHttpServer {
    pub origin: url::Url,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl ScriptedHttpServer {
    pub async fn spawn(responses: Vec<ScriptedResponse>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("scripted downloader listener");
        let address = listener.local_addr().expect("scripted downloader address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.expect("scripted request");
                let request = read_request(&mut stream).await;
                captured.lock().unwrap().push(request);
                if !response.delay.is_zero() {
                    tokio::time::sleep(response.delay).await;
                }
                if !response.bytes.is_empty() {
                    let _ = stream.write_all(&response.bytes).await;
                    let _ = stream.shutdown().await;
                }
            }
        });
        Self {
            origin: url::Url::parse(&format!("http://{address}/")).unwrap(),
            requests,
            task,
        }
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect()
    }
}

impl Drop for ScriptedHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).await.unwrap_or(0);
        if count == 0 {
            return request;
        }
        request.extend_from_slice(&buffer[..count]);
        assert!(request.len() <= 1024 * 1024, "test request exceeded 1 MiB");
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let body_start = header_end + 4;
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if request.len() >= body_start + content_length {
            return request;
        }
    }
}
