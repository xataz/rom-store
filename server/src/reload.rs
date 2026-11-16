use crate::{
    catalog::Catalog,
    models::{ReloadPhase, ReloadStatus},
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Reload {
    state: Arc<Mutex<ReloadStatus>>,
}

impl Reload {
    pub fn status(&self) -> ReloadStatus {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn start(&self, catalog: Catalog) -> ReloadStatus {
        self.schedule(move || catalog.reindex())
    }

    fn schedule(
        &self,
        task: impl FnOnce() -> anyhow::Result<crate::models::IndexReport> + Send + 'static,
    ) -> ReloadStatus {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.status == ReloadPhase::Running {
            return state.clone();
        }
        *state = ReloadStatus {
            status: ReloadPhase::Running,
            ..Default::default()
        };
        let response = state.clone();
        drop(state);
        let manager = self.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(task).await;
            let completed = match result {
                Ok(Ok(report)) => ReloadStatus {
                    status: ReloadPhase::Completed,
                    systems: report.systems,
                    games: report.games,
                    diagnostics: report.diagnostics.len() as u64,
                    error: None,
                },
                result => {
                    tracing::error!("Échec du rechargement du catalogue : {result:?}");
                    ReloadStatus {
                        status: ReloadPhase::Failed,
                        error: Some("Impossible de recharger le catalogue".into()),
                        ..Default::default()
                    }
                }
            };
            *manager.state.lock().unwrap_or_else(|e| e.into_inner()) = completed;
        });
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::IndexReport;
    use std::time::Duration;

    async fn finished(manager: &Reload) -> ReloadStatus {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = manager.status();
                if state.status != ReloadPhase::Running {
                    return state;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn concurrent_requests_join_one_job_without_blocking_async_readers() {
        let manager = Reload::default();
        let (release, blocked) = std::sync::mpsc::channel();
        assert_eq!(
            manager
                .schedule(move || {
                    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(IndexReport {
                        systems: 2,
                        games: 12,
                        diagnostics: Vec::new(),
                        reused: 4,
                        calculated: 1,
                    })
                })
                .status,
            ReloadPhase::Running
        );
        assert_eq!(
            manager
                .schedule(|| panic!("Duplicate request must not execute"))
                .status,
            ReloadPhase::Running
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            tokio::task::yield_now().await;
            assert_eq!(manager.status().status, ReloadPhase::Running);
        })
        .await
        .unwrap();
        release.send(()).unwrap();
        let state = finished(&manager).await;
        assert_eq!(state.status, ReloadPhase::Completed);
        assert_eq!(state.games, 12);
    }

    #[tokio::test]
    async fn panic_is_reported_as_failed_and_does_not_prevent_retry() {
        let manager = Reload::default();
        manager.schedule(|| panic!("private/internal/error"));
        let state = finished(&manager).await;
        assert_eq!(state.status, ReloadPhase::Failed);
        assert!(!state.error.unwrap().contains("private"));
        manager.schedule(|| {
            Ok(IndexReport {
                systems: 0,
                games: 0,
                diagnostics: Vec::new(),
                reused: 0,
                calculated: 0,
            })
        });
        assert_eq!(finished(&manager).await.status, ReloadPhase::Completed);
    }

    #[tokio::test]
    async fn startup_job_is_shared_with_http_readers_and_duplicate_reload_requests() {
        use axum::{body::Body, http::Request};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        std::fs::create_dir(&library).unwrap();
        let catalog = Catalog::new(
            crate::config::Config::new(&library, temp.path().join("catalog.sqlite3"), None)
                .unwrap(),
        )
        .unwrap();
        let manager = Reload::default();
        let (release, blocked) = std::sync::mpsc::channel();
        manager.schedule(move || {
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(IndexReport {
                systems: 0,
                games: 0,
                diagnostics: Vec::new(),
                reused: 0,
                calculated: 0,
            })
        });
        let app = crate::api::router_with_reload(catalog, manager.clone());
        for (method, path) in [
            ("GET", "/api/v1/health"),
            ("GET", "/api/v1/catalog/reload"),
            ("POST", "/api/v1/catalog/reload"),
            ("GET", "/api/v1/catalog/summary"),
            ("GET", "/api/v1/games"),
            ("GET", "/"),
        ] {
            let response = tokio::time::timeout(
                Duration::from_secs(1),
                app.clone().oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                ),
            )
            .await
            .expect("HTTP must stay available during startup indexation")
            .unwrap();
            assert!(response.status().is_success());
            if path == "/api/v1/catalog/reload" {
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                let status: ReloadStatus = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(status.status, ReloadPhase::Running);
            }
        }
        assert_eq!(manager.status().status, ReloadPhase::Running);
        release.send(()).unwrap();
        assert_eq!(finished(&manager).await.status, ReloadPhase::Completed);
    }
}
