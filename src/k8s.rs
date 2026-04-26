//! Async wrappers around `kube` for the dashboard.
//!
//! The UI is kind-agnostic: it sees [`ResourceItem`]s with a name, a
//! short status string, an age, and a pre-serialized YAML body.
//! Adding a new resource kind means a new arm in [`list_resources`]
//! and a one-line entry in `ResourceKind::ALL`.

use anyhow::{anyhow, Result};
use chrono::Utc;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Pod, Secret, Service};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::Resource as K8sResource;
use kube::api::{Api, ListParams};
use kube::config::{Config, Kubeconfig};
use kube::Client;
use serde::Serialize;

use crate::ResourceKind;

/// One row of the resource list as the UI cares about it. Decouples
/// the display from each Kubernetes type's full shape — backends only
/// need name + status + age + YAML.
pub struct ResourceItem {
    pub name: String,
    pub status: String,
    pub age: String,
    /// Pre-serialized YAML for the preview pane. Computed once on
    /// list to avoid re-serializing every time the user moves
    /// selection.
    pub yaml: String,
}

/// Read the current context name from the user's kubeconfig.
pub async fn current_context_name() -> Result<String> {
    let kubeconfig = Kubeconfig::read()?;
    Ok(kubeconfig
        .current_context
        .unwrap_or_else(|| "<unset>".to_string()))
}

/// Build a kube `Client` from the standard kubeconfig discovery path.
async fn make_client() -> Result<Client> {
    let cfg = Config::infer().await.map_err(|e| anyhow!("{e}"))?;
    Client::try_from(cfg).map_err(|e| anyhow!("{e}"))
}

/// List all namespaces, sorted by name. Cluster-scoped, so `Api::all`.
pub async fn list_namespaces() -> Result<Vec<String>> {
    let client = make_client().await?;
    let api: Api<Namespace> = Api::all(client);
    let list = api.list(&ListParams::default()).await?;
    let mut names: Vec<String> = list
        .items
        .into_iter()
        .filter_map(|ns| ns.metadata.name)
        .collect();
    names.sort();
    Ok(names)
}

/// List the resources of `kind` in `namespace` — the one entry point
/// the app calls. Each arm makes the typed `Api<T>::list` call,
/// projects to a `(name, status, age)` triple, and serializes to
/// YAML. Errors bubble up unchanged so the status bar can surface
/// them.
pub async fn list_resources(kind: ResourceKind, namespace: &str) -> Result<Vec<ResourceItem>> {
    let client = make_client().await?;
    match kind {
        ResourceKind::Pods => {
            let api: Api<Pod> = Api::namespaced(client, namespace);
            let list = api.list(&ListParams::default()).await?;
            collect_items(list.items, |p| {
                let phase = p
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.clone())
                    .unwrap_or_else(|| "Unknown".to_string());
                phase
            })
        }
        ResourceKind::Deployments => {
            let api: Api<Deployment> = Api::namespaced(client, namespace);
            let list = api.list(&ListParams::default()).await?;
            collect_items(list.items, |d| {
                let st = d.status.as_ref();
                let ready = st.and_then(|s| s.ready_replicas).unwrap_or(0);
                let desired = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or_default();
                format!("{ready}/{desired}")
            })
        }
        ResourceKind::Services => {
            let api: Api<Service> = Api::namespaced(client, namespace);
            let list = api.list(&ListParams::default()).await?;
            collect_items(list.items, |s| {
                s.spec
                    .as_ref()
                    .and_then(|sp| sp.type_.clone())
                    .unwrap_or_else(|| "ClusterIP".to_string())
            })
        }
        ResourceKind::ConfigMaps => {
            let api: Api<ConfigMap> = Api::namespaced(client, namespace);
            let list = api.list(&ListParams::default()).await?;
            collect_items(list.items, |c| {
                let n = c.data.as_ref().map(|d| d.len()).unwrap_or(0);
                format!("{n} keys")
            })
        }
        ResourceKind::Secrets => {
            let api: Api<Secret> = Api::namespaced(client, namespace);
            let list = api.list(&ListParams::default()).await?;
            collect_items(list.items, |s| {
                s.type_.clone().unwrap_or_else(|| "Opaque".to_string())
            })
        }
    }
}

/// Generic projection: take a Vec of typed objects, project each to
/// `(name, status, age, yaml)`, sort by name, return as
/// `Vec<ResourceItem>`. The status projection is per-kind — passed in
/// as `status_of` so each arm can extract the column it cares about
/// without duplicating the metadata + serialization plumbing.
fn collect_items<T, F>(items: Vec<T>, status_of: F) -> Result<Vec<ResourceItem>>
where
    T: K8sResource + Serialize + HasMeta,
    F: Fn(&T) -> String,
{
    let now = Utc::now();
    let mut rows: Vec<ResourceItem> = items
        .into_iter()
        .map(|obj| {
            let name = obj.meta_name().unwrap_or_else(|| "<no-name>".to_string());
            let age = obj
                .meta_creation()
                .map(|ts| format_age(now.signed_duration_since(ts.0)))
                .unwrap_or_else(|| "?".to_string());
            let status = status_of(&obj);
            let yaml = serde_yaml::to_string(&obj)
                .unwrap_or_else(|e| format!("# YAML serialization failed: {e}\n"));
            ResourceItem {
                name,
                status,
                age,
                yaml,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// Tiny shim trait so `collect_items` doesn't have to know about each
/// type's `metadata` field shape. `kube::Resource` exists but its
/// `meta()` returns the v1 ObjectMeta from `kube`'s own re-export,
/// which complicates lifetimes here — the manual impls below keep the
/// generic function readable.
trait HasMeta {
    fn meta_name(&self) -> Option<String>;
    fn meta_creation(&self) -> Option<&Time>;
}

macro_rules! impl_has_meta {
    ($($t:ty),+ $(,)?) => {
        $(
            impl HasMeta for $t {
                fn meta_name(&self) -> Option<String> { self.metadata.name.clone() }
                fn meta_creation(&self) -> Option<&Time> { self.metadata.creation_timestamp.as_ref() }
            }
        )+
    };
}

impl_has_meta!(Pod, Deployment, Service, ConfigMap, Secret);

/// Format a duration as kubectl-style age: `5m`, `3h`, `12d`.
fn format_age(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}
