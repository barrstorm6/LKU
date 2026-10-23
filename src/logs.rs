use crate::{
    app::{Message, Sink},
    k8s::safe_error,
};
use futures::AsyncReadExt;
use k8s_openapi::api::core::v1::Pod;
use kube::{Api, Client, api::LogParams};
use std::{collections::VecDeque, time::Duration};

pub const CAPACITY: usize = 5_000;
pub const MAX_LINE_BYTES: usize = 16_384;
pub const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub struct LogBuffer {
    pub lines: VecDeque<String>,
    pub revision: u64,
    pub total_lines: u64,
    bytes: usize,
}
impl LogBuffer {
    pub fn append(&mut self, lines: Vec<String>) {
        for mut line in lines {
            if line.len() > MAX_LINE_BYTES {
                let mut end = MAX_LINE_BYTES;
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                line.truncate(end);
            }
            while self.lines.len() >= CAPACITY || self.bytes + line.len() > MAX_BUFFER_BYTES {
                if let Some(old) = self.lines.pop_front() {
                    self.bytes -= old.len();
                } else {
                    break;
                }
            }
            self.bytes += line.len();
            self.lines.push_back(line);
            self.total_lines += 1;
        }
        self.revision += 1;
    }
}

pub struct Viewer {
    pub pod: String,
    pub container: String,
    pub timestamps: bool,
    pub follow: bool,
    pub query: String,
    pub buffer: LogBuffer,
    pub matches: Vec<usize>,
    pub last_revision: u64,
    pub last_query: String,
    pub error: Option<String>,
}
impl Viewer {
    pub fn new(pod: String, container: String) -> Self {
        Self {
            pod,
            container,
            timestamps: false,
            follow: true,
            query: String::new(),
            buffer: LogBuffer::default(),
            matches: vec![],
            last_revision: u64::MAX,
            last_query: String::new(),
            error: None,
        }
    }
    pub fn filter(&mut self) {
        if self.last_revision == self.buffer.revision && self.last_query == self.query {
            return;
        }
        let query = self.query.to_lowercase();
        self.matches = self
            .buffer
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| query.is_empty() || line.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        self.last_revision = self.buffer.revision;
        self.last_query.clone_from(&self.query);
    }
}

pub async fn stream(
    client: Client,
    namespace: String,
    pod: String,
    container: String,
    timestamps: bool,
    id: u64,
    sink: Sink,
) {
    let api: Api<Pod> = Api::namespaced(client, &namespace);
    let mut reconnect = false;
    let mut last_stamp: Option<String> = None;
    let mut delay = 1;
    loop {
        let params = LogParams {
            container: Some(container.clone()),
            follow: true,
            timestamps: true,
            tail_lines: if reconnect && last_stamp.is_some() {
                None
            } else {
                Some(500)
            },
            since_time: if reconnect {
                last_stamp.as_deref().and_then(|stamp| stamp.parse().ok())
            } else {
                None
            },
            ..Default::default()
        };
        let result =
            tokio::time::timeout(Duration::from_secs(20), api.log_stream(&pod, &params)).await;
        match result {
            Ok(Ok(reader)) => {
                if !sink.send(Message::LogStatus(id, None)).await {
                    return;
                }
                delay = 1;
                let mut reader = reader;
                let mut chunk = [0_u8; 8192];
                let mut line = Vec::with_capacity(256);
                let mut batch = Vec::new();
                let mut tick = tokio::time::interval(Duration::from_millis(50));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        read = reader.read(&mut chunk) => {
                            match read {
                                Ok(n) if n > 0 => {
                                    for byte in &chunk[..n] {
                                        if *byte == b'\n' {
                                            if line.last() == Some(&b'\r') { line.pop(); }
                                            batch.push(display_line(&line,timestamps,&mut last_stamp));
                                            line.clear();
                                        } else if line.len() < MAX_LINE_BYTES { line.push(*byte); }
                                        if batch.len() >= 8 && !sink.send(Message::LogLines(id,std::mem::take(&mut batch))).await { return; }
                                    }
                                }
                                _ => {
                                    if !line.is_empty() { batch.push(display_line(&line,timestamps,&mut last_stamp)); }
                                    if !batch.is_empty() && !sink.send(Message::LogLines(id, batch)).await { return; }
                                    break;
                                }
                            }
                        }
                        _ = tick.tick() => {
                            if !batch.is_empty() && !sink.send(Message::LogLines(id, std::mem::take(&mut batch))).await { return; }
                        }
                    }
                }
            }
            Ok(Err(error)) => {
                let terminal =
                    matches!(&error, kube::Error::Api(e) if matches!(e.code,401|403|404));
                if !sink
                    .send(Message::LogStatus(id, Some(safe_error(&error))))
                    .await
                    || terminal
                {
                    return;
                }
            }
            Err(_) => {
                if !sink
                    .send(Message::LogStatus(
                        id,
                        Some("Logs timed out. Reconnecting…".into()),
                    ))
                    .await
                {
                    return;
                }
            }
        }
        if !sink
            .send(Message::LogStatus(
                id,
                Some("Log stream ended. Reconnecting…".into()),
            ))
            .await
        {
            return;
        }
        reconnect = true;
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}

fn display_line(bytes: &[u8], timestamps: bool, last_stamp: &mut Option<String>) -> String {
    let line = String::from_utf8_lossy(bytes);
    if let Some((stamp, text)) = line.split_once(' ') {
        *last_stamp = Some(stamp.to_owned());
        if !timestamps {
            return text.to_owned();
        }
    }
    line.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn actual_log_api_stream_is_cancellable_and_strips_optional_timestamps() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            let n = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            assert!(request.contains("/namespaces/test/pods/pod/log?"));
            assert!(request.contains("container=app"));
            assert!(request.contains("follow=true"));
            let body = "2026-10-23T00:00:00Z first\n2026-10-23T00:00:01Z второй\n";
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        });
        let client = Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let sink = Sink {
            generation: 1,
            tx,
            ctx: egui::Context::default(),
        };
        let task = tokio::spawn(stream(
            client,
            "test".into(),
            "pod".into(),
            "app".into(),
            false,
            7,
            sink,
        ));
        let lines = tokio::time::timeout(Duration::from_secs(5), async {
            let mut lines = Vec::new();
            while lines.len() < 2 {
                if let Some(crate::app::Envelope {
                    message: Message::LogLines(7, batch),
                    ..
                }) = rx.recv().await
                {
                    lines.extend(batch);
                }
            }
            lines
        })
        .await
        .unwrap();
        assert_eq!(lines, vec!["first", "второй"]);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        server.await.unwrap();
    }
    #[test]
    fn bounded_buffer_and_filter() {
        let mut view = Viewer::new("pod".into(), "app".into());
        view.buffer
            .append((0..6000).map(|i| format!("line {i}")).collect());
        assert_eq!(view.buffer.lines.len(), CAPACITY);
        assert_eq!(view.buffer.lines.front().unwrap(), "line 1000");
        view.query = "LINE 5999".into();
        view.filter();
        assert_eq!(view.matches.len(), 1);
        view.buffer.append(vec!["я".repeat(MAX_LINE_BYTES)]);
        assert!(view.buffer.lines.back().unwrap().len() <= MAX_LINE_BYTES);
    }
}
