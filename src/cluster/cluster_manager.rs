/*
 * Copyright 2020 Google LLC
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::sync::Arc;

// We use a parking_lot since it's significantly faster under low contention
// and we will need to acquire a read lock with every packet that is processed
// to be able to capture the current endpoint state and pass it to Filters.
use parking_lot::RwLock;
use slog::{debug, o, warn, Logger};

use prometheus::{Registry, Result as MetricsResult};
use tokio::sync::{mpsc, watch};

use crate::cluster::Endpoint;
use crate::config::{Endpoints, UpstreamEndpoints};
use crate::xds::ads_client::ClusterUpdate;

use super::metrics::Metrics;

pub(crate) type SharedClusterManager = Arc<RwLock<ClusterManager>>;

/// ClusterManager knows about all clusters and endpoints.
pub(crate) struct ClusterManager {
    metrics: Metrics,
    endpoints: Option<Endpoints>,
}

/// InitializeError is returned with an error message if the
/// [`ClusterManager`] fails to initialize properly.
#[derive(Debug, thiserror::Error)]
pub enum InitializeError {
    #[error("{:?}", .0)]
    Message(String),
}

impl ClusterManager {
    fn new(metrics_registry: &Registry, endpoints: Option<Endpoints>) -> MetricsResult<Self> {
        let metrics = Metrics::new(metrics_registry)?;
        Ok(Self { metrics, endpoints })
    }

    fn update(&mut self, endpoints: Option<Endpoints>) {
        self.endpoints = endpoints;
    }

    /// Returns all endpoints known at the time of invocation.
    /// Returns `None` if there are no endpoints.
    pub fn get_all_endpoints(&self) -> Option<UpstreamEndpoints> {
        self.endpoints.clone().map(|ep| ep.into())
    }

    /// Returns a ClusterManager backed by the fixed set of clusters provided in the config.
    pub fn fixed(
        metrics_registry: &Registry,
        endpoints: Endpoints,
    ) -> MetricsResult<SharedClusterManager> {
        let cm = Self::new(metrics_registry, Some(endpoints))?;
        // Set the endpoints count metrics.
        cm.metrics.active_endpoints.set(
            cm.endpoints
                .as_ref()
                .map(|ep| ep.as_ref().len())
                .unwrap_or_default() as i64,
        );
        Ok(Arc::new(RwLock::new(cm)))
    }

    /// Returns a ClusterManager backed by a set of XDS servers.
    /// This function starts an XDS client in the background that talks to
    /// one of the provided servers.
    /// Multiple management servers can be provided for redundancy - the servers will be
    /// connected to in turn only in the case of failure.
    /// The set of clusters is continuously updated based on responses
    /// from the XDS server.
    /// The returned contains the XDS client's execution result after termination.
    pub fn dynamic(
        base_logger: Logger,
        metrics_registry: &Registry,
        cluster_update: ClusterUpdate,
        cluster_updates_rx: mpsc::Receiver<ClusterUpdate>,
        shutdown_rx: watch::Receiver<()>,
    ) -> MetricsResult<SharedClusterManager> {
        let log = base_logger.new(o!("source" => "cluster::ClusterManager"));

        let cluster_manager = Self::new(
            metrics_registry,
            Self::create_endpoints_from_update(&cluster_update),
        )?;
        let metrics = cluster_manager.metrics.clone();
        let cluster_manager = Arc::new(RwLock::new(cluster_manager));

        Self::update_cluster_update_metrics(&metrics, &cluster_update);

        // Start a task in the background to receive cluster updates
        // and update the cluster manager's cluster set in turn.
        Self::spawn_updater(
            log.clone(),
            metrics,
            cluster_manager.clone(),
            cluster_updates_rx,
            shutdown_rx,
        );

        Ok(cluster_manager)
    }

    fn update_cluster_update_metrics(metrics: &Metrics, update: &ClusterUpdate) {
        metrics.active_clusters.set(update.len() as i64);
        metrics.active_endpoints.set(
            Self::create_endpoints_from_update(update)
                .map(|ep| ep.as_ref().len() as i64)
                .unwrap_or_default(),
        )
    }

    fn create_endpoints_from_update(update: &ClusterUpdate) -> Option<Endpoints> {
        // NOTE: We don't currently have support for consuming multiple clusters
        // so here gather all endpoints into the same set, ignoring what cluster they
        // belong to.
        let endpoints = update
            .iter()
            .fold(vec![], |mut endpoints, (_name, cluster)| {
                let cluster_endpoints = cluster
                    .localities
                    .iter()
                    .map(|(_, endpoints)| {
                        endpoints
                            .endpoints
                            .iter()
                            .map(|ep| Endpoint::from_address(ep.address))
                    })
                    .flatten();
                endpoints.extend(cluster_endpoints);

                endpoints
            });

        match Endpoints::new(endpoints) {
            Ok(endpoints) => Some(endpoints),
            Err(_empty_list_error) => None,
        }
    }

    /// Spawns a task to run a loop that receives cluster updates
    /// and updates the ClusterManager's state in turn.
    fn spawn_updater(
        log: Logger,
        metrics: Metrics,
        cluster_manager: Arc<RwLock<ClusterManager>>,
        mut cluster_updates_rx: mpsc::Receiver<ClusterUpdate>,
        mut shutdown_rx: watch::Receiver<()>,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    update = cluster_updates_rx.recv() => {
                        match update {
                            Some(update) => {
                                Self::update_cluster_update_metrics(&metrics, &update);
                                let update = Self::create_endpoints_from_update(&update);
                                debug!(log, "Received a cluster update.");
                                cluster_manager.write().update(update);
                            }
                            None => {
                                warn!(log, "Exiting cluster update receive loop because the sender dropped the channel.");
                                return;
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        debug!(log, "Exiting cluster update receive loop because a shutdown signal was received.");
                        return;
                    },
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::ClusterManager;
    use crate::cluster::{Cluster, Endpoint, LocalityEndpoints};
    use crate::config::Endpoints;
    use crate::test_utils::logger;
    use prometheus::Registry;
    use tokio::sync::{mpsc, watch};

    #[test]
    fn static_cluster_manager_metrics() {
        let cm = ClusterManager::fixed(
            &Registry::default(),
            Endpoints::new(vec![
                Endpoint::from_address("127.0.0.1:80".parse().unwrap()),
                Endpoint::from_address("127.0.0.1:81".parse().unwrap()),
            ])
            .unwrap(),
        )
        .unwrap();
        let metrics = &cm.read().metrics;
        assert_eq!(2, metrics.active_endpoints.get());
        assert_eq!(0, metrics.active_clusters.get());
    }

    #[tokio::test]
    async fn dynamic_cluster_manager_metrics() {
        let (update_tx, update_rx) = mpsc::channel(3);
        let (_shutdown_tx, shutdown_rx) = watch::channel(());
        let cm = ClusterManager::dynamic(
            logger(),
            &Registry::default(),
            vec![(
                "cluster-1".into(),
                Cluster {
                    localities: vec![(
                        None,
                        LocalityEndpoints {
                            endpoints: vec![
                                Endpoint::from_address("127.0.0.1:80".parse().unwrap()),
                                Endpoint::from_address("127.0.0.1:81".parse().unwrap()),
                            ],
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
            )]
            .into_iter()
            .collect(),
            update_rx,
            shutdown_rx,
        )
        .unwrap();

        // Initialization metrics
        {
            let metrics = &cm.read().metrics;
            assert_eq!(2, metrics.active_endpoints.get());
            assert_eq!(1, metrics.active_clusters.get());
        }

        let update = vec![
            (
                "cluster-1".into(),
                Cluster {
                    localities: vec![(
                        None,
                        LocalityEndpoints {
                            endpoints: vec![Endpoint::from_address(
                                "127.0.0.1:80".parse().unwrap(),
                            )],
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
            ),
            (
                "cluster-2".into(),
                Cluster {
                    localities: vec![(
                        None,
                        LocalityEndpoints {
                            endpoints: vec![
                                Endpoint::from_address("127.0.0.1:82".parse().unwrap()),
                                Endpoint::from_address("127.0.0.1:83".parse().unwrap()),
                            ],
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
            ),
        ]
        .into_iter()
        .collect();
        update_tx.send(update).await.unwrap();

        // Check updated metrics
        tokio::time::timeout(std::time::Duration::from_secs(3), async move {
            // Wait for the update to be processed. Here just poll until there's
            // a change we expect (or we will timeout from the enclosing future eventually.
            loop {
                {
                    let metrics = &cm.read().metrics;
                    if metrics.active_endpoints.get() == 3 {
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
            }

            // Verify the new metrics are correct.
            {
                let metrics = &cm.read().metrics;
                assert_eq!(3, metrics.active_endpoints.get());
                assert_eq!(2, metrics.active_clusters.get());
            }
        })
        .await
        .unwrap();
    }
}
