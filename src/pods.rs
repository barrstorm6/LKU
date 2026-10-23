use k8s_openapi::api::core::v1::Pod;
use kube::runtime::watcher::Event;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Unhealthy,
    Transitional,
}

#[derive(Clone, Debug)]
pub struct PodView {
    pub uid: String,
    pub name: String,
    pub search_name: String,
    pub namespace: String,
    pub health: Health,
    pub status: String,
    pub ready: usize,
    pub total: usize,
    pub restarts: i32,
    pub ip: String,
    pub node: String,
    pub containers: Vec<String>,
}

impl From<Pod> for PodView {
    fn from(pod: Pod) -> Self {
        let status = pod.status.as_ref();
        let spec = pod.spec.as_ref();
        let containers: Vec<String> = spec
            .map(|s| s.containers.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        let regular = status
            .and_then(|s| s.container_statuses.as_deref())
            .unwrap_or_default();
        let init = status
            .and_then(|s| s.init_container_statuses.as_deref())
            .unwrap_or_default();
        let ready = regular.iter().filter(|c| c.ready).count();
        let phase = status.and_then(|s| s.phase.as_deref()).unwrap_or("Unknown");
        let failing = regular.iter().chain(init.iter()).find_map(|c| {
            let state = c.state.as_ref()?;
            if let Some(waiting) = &state.waiting {
                let reason = waiting.reason.as_deref()?;
                if matches!(
                    reason,
                    "CrashLoopBackOff"
                        | "ImagePullBackOff"
                        | "ErrImagePull"
                        | "CreateContainerConfigError"
                        | "CreateContainerError"
                        | "InvalidImageName"
                        | "RunContainerError"
                ) {
                    return Some(reason.to_owned());
                }
            }
            state
                .terminated
                .as_ref()
                .filter(|t| t.exit_code != 0)
                .map(|t| {
                    t.reason
                        .clone()
                        .unwrap_or_else(|| "Container failed".into())
                })
        });
        let is_ready = status
            .and_then(|s| s.conditions.as_ref())
            .is_some_and(|conditions| {
                conditions
                    .iter()
                    .any(|c| c.type_ == "Ready" && c.status == "True")
            });
        let (health, text) = if pod.metadata.deletion_timestamp.is_some() {
            (Health::Transitional, "Terminating".into())
        } else if phase == "Succeeded" {
            (Health::Transitional, "Completed".into())
        } else if phase == "Failed" || failing.is_some() {
            (
                Health::Unhealthy,
                failing.unwrap_or_else(|| "Failed".into()),
            )
        } else if phase == "Running"
            && is_ready
            && ready == containers.len()
            && !containers.is_empty()
        {
            (Health::Healthy, "Ready".into())
        } else {
            (
                Health::Transitional,
                if phase == "Running" {
                    "Not ready"
                } else {
                    phase
                }
                .into(),
            )
        };
        let name = pod.metadata.name.unwrap_or_default();
        Self {
            uid: pod.metadata.uid.unwrap_or_else(|| name.clone()),
            search_name: name.to_lowercase(),
            name,
            namespace: pod.metadata.namespace.unwrap_or_default(),
            health,
            status: text,
            ready,
            total: containers.len(),
            restarts: regular
                .iter()
                .chain(init.iter())
                .map(|c| c.restart_count)
                .sum(),
            ip: status.and_then(|s| s.pod_ip.clone()).unwrap_or_default(),
            node: spec.and_then(|s| s.node_name.clone()).unwrap_or_default(),
            containers,
        }
    }
}

#[derive(Default)]
pub struct PodStore {
    pub live: BTreeMap<String, PodView>,
    staging: BTreeMap<String, PodView>,
}

impl PodStore {
    pub fn apply(&mut self, event: Event<PodView>) {
        match event {
            Event::Init => self.staging.clear(),
            Event::InitApply(pod) => {
                self.staging.insert(pod.name.clone(), pod);
            }
            Event::InitDone => {
                self.live = std::mem::take(&mut self.staging);
            }
            Event::Apply(pod) => {
                self.live.insert(pod.name.clone(), pod);
            }
            Event::Delete(pod) => {
                if self
                    .live
                    .get(&pod.name)
                    .is_some_and(|current| current.uid == pod.uid)
                {
                    self.live.remove(&pod.name);
                }
            }
        }
    }
    pub fn matches(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        self.live
            .values()
            .filter(|p| p.search_name.contains(&query))
            .map(|p| p.name.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pod(phase: &str, ready: bool) -> Pod {
        serde_json::from_value(serde_json::json!({"metadata":{"name":"API-Test","uid":"one"},"spec":{"containers":[{"name":"app"}]},"status":{"phase":phase,"conditions":[{"type":"Ready","status":if ready {"True"} else {"False"}}],"containerStatuses":[{"name":"app","image":"app","imageID":"","ready":ready,"restartCount":0}]}})).unwrap()
    }
    #[test]
    fn health_mapping() {
        assert_eq!(PodView::from(pod("Running", true)).health, Health::Healthy);
        for phase in ["Running", "Pending", "Succeeded", "Unknown"] {
            assert_eq!(
                PodView::from(pod(phase, false)).health,
                Health::Transitional
            );
        }
        assert_eq!(
            PodView::from(pod("Failed", false)).health,
            Health::Unhealthy
        );
        let mut crash = pod("Running", false);
        crash
            .status
            .as_mut()
            .unwrap()
            .container_statuses
            .as_mut()
            .unwrap()[0]
            .state = Some(
            serde_json::from_value(serde_json::json!({"waiting":{"reason":"CrashLoopBackOff"}}))
                .unwrap(),
        );
        assert_eq!(PodView::from(crash).health, Health::Unhealthy);
    }
    #[test]
    fn incremental_and_relist() {
        let mut store = PodStore::default();
        let view = PodView::from(pod("Running", true));
        store.apply(Event::Apply(view.clone()));
        assert_eq!(store.matches("api-TEST").len(), 1);
        assert!(store.matches("missing").is_empty());
        store.apply(Event::Init);
        assert_eq!(store.live.len(), 1);
        store.apply(Event::InitApply(view.clone()));
        store.apply(Event::InitDone);
        assert_eq!(store.live.len(), 1);
        store.apply(Event::Delete(view));
        assert!(store.live.is_empty());
        store.apply(Event::Init);
        store.apply(Event::InitDone);
        assert!(store.live.is_empty());
    }
    #[test]
    #[ignore = "run with --release -- --ignored --nocapture for measured search timings"]
    fn benchmark_search() {
        for count in [1_000, 10_000] {
            let mut store = PodStore::default();
            for n in 0..count {
                let mut view = PodView::from(pod("Running", true));
                view.name = format!("pod-{n:05}");
                view.search_name = view.name.clone();
                store.apply(Event::Apply(view));
            }
            let mut samples = Vec::new();
            for _ in 0..200 {
                let start = std::time::Instant::now();
                std::hint::black_box(store.matches(std::hint::black_box("pod-0")));
                samples.push(start.elapsed());
            }
            samples.sort();
            println!("{count} pods: search p95 {:?}", samples[189]);
        }
    }
}
