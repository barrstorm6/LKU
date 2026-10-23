use crate::{config::Connection, pods::PodView};
use futures::StreamExt;
use k8s_openapi::api::core::v1::{Namespace, Pod};
use kube::{
    Api, Client, Config,
    api::ListParams,
    config::{KubeConfigOptions, Kubeconfig},
    runtime::watcher,
};
use std::{path::PathBuf, time::Duration};

pub struct Candidate {
    pub path: PathBuf,
    pub config: Kubeconfig,
    pub contexts: Vec<String>,
    pub selected: String,
    pub replacing: Option<String>,
}

impl Candidate {
    pub fn read(path: PathBuf, replacing: Option<String>) -> Result<Self, String> {
        let config = Kubeconfig::read_from(&path).map_err(|_| {
            "Cannot read kubeconfig. Check the path, permissions, and YAML syntax.".to_owned()
        })?;
        Self::new(path, config, replacing)
    }
    fn new(path: PathBuf, config: Kubeconfig, replacing: Option<String>) -> Result<Self, String> {
        let contexts: Vec<_> = config
            .contexts
            .iter()
            .filter(|c| c.context.is_some())
            .map(|c| c.name.clone())
            .collect();
        if contexts.is_empty() {
            return Err("Kubeconfig contains no contexts.".into());
        }
        let selected = config
            .current_context
            .as_ref()
            .filter(|c| contexts.contains(c))
            .cloned()
            .unwrap_or_else(|| contexts[0].clone());
        Ok(Self {
            path,
            config,
            contexts,
            selected,
            replacing,
        })
    }
    pub fn validate(&self) -> Result<Option<String>, String> {
        let context = self
            .config
            .contexts
            .iter()
            .find(|c| c.name == self.selected)
            .and_then(|c| c.context.as_ref())
            .ok_or("Select a valid context.")?;
        let cluster = self
            .config
            .clusters
            .iter()
            .find(|c| c.name == context.cluster)
            .and_then(|c| c.cluster.as_ref())
            .ok_or("The context references a missing cluster.")?;
        if cluster.insecure_skip_tls_verify == Some(true) {
            return Err("This kubeconfig disables TLS verification. Configure a trusted CA certificate before connecting.".into());
        }
        let server = cluster
            .server
            .as_deref()
            .ok_or("The cluster has no API server address.")?;
        if !server.starts_with("https://") {
            return Err("The API server must use HTTPS with certificate verification.".into());
        }
        if let Some(user) = &context.user {
            let auth = self
                .config
                .auth_infos
                .iter()
                .find(|a| &a.name == user)
                .and_then(|a| a.auth_info.as_ref())
                .ok_or("The context references a missing user.")?;
            // Legacy auth-provider configurations may also execute commands. Require a supported exec plugin instead.
            if auth.auth_provider.is_some() {
                return Err("Legacy auth-provider authentication is unsupported. Use a kubeconfig with an exec credential plugin.".into());
            }
            if let Some(exec) = &auth.exec {
                return Ok(Some(
                    exec.command
                        .clone()
                        .ok_or("The credential plugin has no command.")?,
                ));
            }
        }
        Ok(None)
    }
    pub fn namespace(&self) -> String {
        self.config
            .contexts
            .iter()
            .find(|c| c.name == self.selected)
            .and_then(|c| c.context.as_ref())
            .and_then(|c| c.namespace.clone())
            .unwrap_or_else(|| "default".into())
    }
}

pub struct Connected {
    pub client: Client,
    pub connection: Connection,
    pub namespaces: Vec<String>,
    pub restricted: bool,
}

pub async fn connect(
    candidate: Candidate,
    mut connection: Connection,
) -> Result<Connected, String> {
    let config = Config::from_custom_kubeconfig(candidate.config, &KubeConfigOptions { context: Some(candidate.selected), ..Default::default() })
        .await.map_err(|_| "Could not initialize credentials or certificates. Check kubeconfig and credential plugin installation.".to_owned())?;
    let mut config = config;
    config.connect_timeout = Some(Duration::from_secs(10));
    config.read_timeout = Some(Duration::from_secs(330));
    config.write_timeout = Some(Duration::from_secs(15));
    let client = Client::try_from(config).map_err(|_| {
        "Could not initialize the TLS client. Check certificates and authentication.".to_owned()
    })?;
    let (namespaces, restricted) = list_namespaces(client.clone()).await?;
    if !restricted && !namespaces.is_empty() && !namespaces.contains(&connection.namespace) {
        connection.namespace = candidate_namespace_fallback(&namespaces);
    }
    Ok(Connected {
        client,
        connection,
        namespaces,
        restricted,
    })
}

async fn list_namespaces(client: Client) -> Result<(Vec<String>, bool), String> {
    let api: Api<Namespace> = Api::all(client);
    let (mut namespaces, restricted) =
        match tokio::time::timeout(Duration::from_secs(20), api.list(&ListParams::default())).await
        {
            Ok(Ok(list)) => (
                list.items
                    .into_iter()
                    .filter_map(|n| n.metadata.name)
                    .collect::<Vec<_>>(),
                false,
            ),
            Ok(Err(kube::Error::Api(error))) if error.code == 403 => (vec![], true),
            Ok(Err(error)) => return Err(safe_error(&error)),
            Err(_) => {
                return Err("Connection timed out. Check network, VPN, and the API server.".into());
            }
        };
    namespaces.sort();
    Ok((namespaces, restricted))
}

fn candidate_namespace_fallback(namespaces: &[String]) -> String {
    if namespaces.iter().any(|n| n == "default") {
        "default".into()
    } else {
        namespaces[0].clone()
    }
}

pub fn safe_error(error: &kube::Error) -> String {
    match error {
        kube::Error::Api(response) => match response.code {
            401 => "Authentication failed (401). Refresh your credentials and retry.".into(),
            403 => "Access denied (403). Ask for read permission in this namespace.".into(),
            404 => "Resource no longer exists (404). Check the namespace or reopen the Pod.".into(),
            429 => "API request limit reached. Reconnecting shortly.".into(),
            _ => format!(
                "Kubernetes API error ({}). Check server availability and permissions.",
                response.code
            ),
        },
        _ => "Request failed. Check network, VPN, credentials, and TLS certificates.".into(),
    }
}

pub async fn watch_pods(client: Client, namespace: String, sink: crate::app::Sink) {
    let api: Api<Pod> = Api::namespaced(client, &namespace);
    let stream = watcher(api, watcher::Config::default().timeout(290)).default_backoff();
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        let message = match event {
            Ok(event) => crate::app::Message::Pods(match event {
                watcher::Event::Apply(p) => watcher::Event::Apply(PodView::from(p)),
                watcher::Event::Delete(p) => watcher::Event::Delete(PodView::from(p)),
                watcher::Event::Init => watcher::Event::Init,
                watcher::Event::InitApply(p) => watcher::Event::InitApply(PodView::from(p)),
                watcher::Event::InitDone => watcher::Event::InitDone,
            }),
            Err(error) => {
                let text = match error {
                    watcher::Error::InitialListFailed(e)
                    | watcher::Error::WatchStartFailed(e)
                    | watcher::Error::WatchFailed(e) => safe_error(&e),
                    _ => "Pod watch interrupted. Reconnecting…".into(),
                };
                crate::app::Message::PodError(text)
            }
        };
        if !sink.send(message).await {
            break;
        }
    }
}

use kube::runtime::WatchStreamExt;

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(text: &str) -> Result<Candidate, String> {
        Candidate::new(
            PathBuf::from("config"),
            Kubeconfig::from_yaml(text).map_err(|_| "Invalid YAML")?,
            None,
        )
    }
    const YAML: &str = "apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster:\n    server: https://localhost:6443\ncontexts:\n- name: a\n  context:\n    cluster: c\n    namespace: test\n- name: b\n  context:\n    cluster: c\ncurrent-context: b\n";
    #[test]
    fn parsing_and_selection() {
        let mut c = candidate(YAML).unwrap();
        assert_eq!(c.selected, "b");
        assert_eq!(c.namespace(), "default");
        c.selected = "a".into();
        assert_eq!(c.namespace(), "test");
        assert_eq!(c.validate().unwrap(), None);
        assert!(candidate("contexts: [").is_err());
        assert!(candidate("").is_err());
        let c = candidate(&YAML.replace("current-context: b", "current-context: missing")).unwrap();
        assert_eq!(c.selected, "a");
    }
    #[test]
    fn rejects_insecure_and_requires_exec_trust() {
        let insecure = YAML.replace(
            "server: https://localhost:6443",
            "server: https://localhost:6443\n    insecure-skip-tls-verify: true",
        );
        assert!(candidate(&insecure).unwrap().validate().is_err());
        let exec = YAML.replace(
            "cluster: c\ncurrent-context",
            "cluster: c\n    user: u\ncurrent-context",
        ) + "users:\n- name: u\n  user:\n    exec:\n      command: credential-helper\n";
        assert_eq!(
            candidate(&exec).unwrap().validate().unwrap(),
            Some("credential-helper".into())
        );
    }

    #[tokio::test]
    async fn api_namespace_forbidden_still_lists_and_watches_pods() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for request in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = vec![0; 8192];
                let n = socket.read(&mut data).await.unwrap();
                let request_line = String::from_utf8_lossy(&data[..n]);
                let (status, body) = match request {
                    0 => {
                        assert!(request_line.contains("/api/v1/namespaces?"));
                        (403,r#"{"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Forbidden","message":"fixture","code":403}"#.to_owned())
                    }
                    1 => {
                        assert!(request_line.contains("/namespaces/test/pods?"));
                        (200,r#"{"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"name":"fixture-pod","namespace":"test","uid":"1","resourceVersion":"1"}}]}"#.to_owned())
                    }
                    _ => {
                        assert!(request_line.contains("watch=true"));
                        (200,"{\"type\":\"DELETED\",\"object\":{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{\"name\":\"fixture-pod\",\"namespace\":\"test\",\"uid\":\"1\",\"resourceVersion\":\"2\"}}}\n".to_owned())
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        // HTTP is used only by this isolated loopback fixture, never by production config validation.
        let client =
            Client::try_from(Config::new(format!("http://{address}").parse().unwrap())).unwrap();
        let (namespaces, restricted) = list_namespaces(client.clone()).await.unwrap();
        assert!(restricted);
        assert!(namespaces.is_empty());
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let sink = crate::app::Sink {
            generation: 1,
            tx,
            ctx: egui::Context::default(),
        };
        let task = tokio::spawn(watch_pods(client, "test".into(), sink));
        let mut store = crate::pods::PodStore::default();
        let mut initialized = false;
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(envelope) = rx.recv().await {
                match envelope.message {
                    crate::app::Message::Pods(event) => {
                        let done = matches!(event, watcher::Event::InitDone);
                        let deleted = matches!(event, watcher::Event::Delete(_));
                        store.apply(event);
                        if done {
                            initialized = true;
                            assert_eq!(store.live.len(), 1);
                        }
                        if deleted {
                            assert!(store.live.is_empty());
                            break;
                        }
                    }
                    crate::app::Message::PodError(error) => panic!("{error}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert!(initialized);
        task.abort();
        server.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires KUBE_TEST_CONFIG pointing to a disposable local cluster"]
    async fn disposable_cluster_read_only_smoke() {
        let path = std::env::var_os("KUBE_TEST_CONFIG")
            .expect("Set KUBE_TEST_CONFIG to a disposable cluster kubeconfig");
        let candidate = Candidate::read(path.into(), None).unwrap();
        assert!(
            candidate.validate().unwrap().is_none(),
            "Fixture must not require an exec program"
        );
        let connection = Connection {
            id: "test".into(),
            name: "test".into(),
            kubeconfig_path: candidate.path.clone(),
            context: candidate.selected.clone(),
            namespace: candidate.namespace(),
        };
        let connected = connect(candidate, connection).await.unwrap();
        let pods: Api<Pod> = Api::namespaced(connected.client, &connected.connection.namespace);
        let list = pods.list(&ListParams::default()).await.unwrap();
        if let Some(pod) = list.items.first() {
            let name = pod.metadata.name.as_deref().unwrap();
            let container = pod.spec.as_ref().unwrap().containers[0].name.clone();
            let params = kube::api::LogParams {
                container: Some(container),
                tail_lines: Some(10),
                ..Default::default()
            };
            let _ = pods.logs(name, &params).await.unwrap();
        }
    }
}
