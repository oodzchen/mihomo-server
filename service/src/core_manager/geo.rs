//! Geo/resource inspection, seed installation and online updates.
use super::*;

impl CoreManager {
    pub async fn geo_settings(&self) -> Result<crate::geo_settings::Snapshot> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            CommandMessage::ReadGeoSettings,
            "Geo settings read cancelled during shutdown",
        )
        .await
    }

    #[cfg(unix)]
    pub async fn geo_seed_info(&self, name: String) -> Result<crate::geo_update::SeedInfo> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::GeoSeedInfo { name, reply },
            "Geo seed read cancelled during shutdown",
        )
        .await
    }

    #[cfg(unix)]
    pub async fn install_geo_seed(
        &self,
        request: crate::geo_update::InstallRequest,
    ) -> Result<crate::geo_update::Receipt> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::InstallGeoSeed { request, reply },
            "Geo installation response cancelled during shutdown",
        )
        .await
    }

    #[cfg(unix)]
    pub async fn geo_online_info(&self, name: String) -> Result<crate::geo_online::Info> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::GeoOnlineInfo { name, reply },
            "Geo source inspection cancelled during shutdown",
        )
        .await
    }

    #[cfg(unix)]
    pub async fn update_geo_online(&self, request: crate::geo_online::Request) -> Result<crate::geo_update::Receipt> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::UpdateGeoOnline { request, reply },
            "Geo online update response cancelled during shutdown",
        )
        .await
    }

    pub async fn validate_geo(&self, name: String) -> Result<crate::geo_validation::Validation> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::ValidateGeo { name, reply },
            "Geo validation cancelled during shutdown",
        )
        .await
    }

    pub async fn resource_inventory(&self) -> Result<crate::resource_inventory::Inventory> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(CommandMessage::ReadResources, "resource read cancelled during shutdown")
            .await
    }
}

impl Actor {
    #[cfg(unix)]
    pub(super) async fn geo_download_route(
        &mut self,
        choice: crate::geo_online::RouteChoice,
        config: &Mapping,
    ) -> Result<crate::core_release::Route> {
        use crate::geo_online::RouteChoice;
        match choice {
            RouteChoice::Direct => Ok(crate::core_release::Route::Direct),
            RouteChoice::System => Ok(crate::core_release::Route::System),
            RouteChoice::Managed => {
                let snapshot = self.status.borrow().clone();
                ensure!(
                    snapshot.phase == CorePhase::Running && self.process.is_some(),
                    "managed Geo route requires a running core"
                );
                let core = tokio::select! {biased;
                    _ = closing(&mut self.shutdown) => bail!("managed Geo route cancelled during shutdown"),
                    result = timeout(Duration::from_secs(3), self.client.get_base_config()) => result.context("managed Geo route query timed out")??,
                };
                ensure!(
                    same_proxy_snapshot(&snapshot, &self.status.borrow()),
                    "managed Geo route changed during inspection"
                );
                crate::proxy_access::verify_ports(config, &core)?;
                let proxy = crate::remote::ManagedProxy::from_core(&core, config)?;
                Ok(crate::core_release::Route::Managed {
                    proxy,
                    snapshot: Box::new(snapshot),
                    state: self.status.subscribe(),
                })
            }
        }
    }

    #[cfg(unix)]
    pub(super) async fn verify_prepared_geo(
        &mut self,
        mut prepared: crate::geo_update::Prepared,
        name: &str,
    ) -> Result<crate::geo_update::Prepared> {
        if crate::dat_validation::DAT_FILES.contains(&name) {
            let (next, probe) = tokio::task::spawn_blocking(move || {
                let probe = prepared.probe()?;
                Ok::<_, anyhow::Error>((prepared, probe))
            })
            .await
            .context("DAT probe staging worker failed")??;
            prepared = next;
            for loader in ["standard", "memconservative"] {
                for matcher in ["mph", "succinct"] {
                    tokio::fs::write(&probe.config, probe.config_for(loader, matcher)).await?;
                    crate::validation::validate(
                        &self.options.binary,
                        &probe.directory,
                        &probe.config,
                        &mut self.shutdown,
                        Duration::from_secs(15),
                    )
                    .await
                    .with_context(|| {
                        format!("isolated Mihomo DAT compatibility probe failed for {loader}/{matcher}")
                    })?;
                }
            }
            probe.verify_input()?;
            prepared.mark_core_load_verified();
        }
        Ok(prepared)
    }

    #[cfg(unix)]
    pub(super) async fn publish_prepared_geo(
        &mut self,
        prepared: crate::geo_update::Prepared,
        name: &str,
    ) -> Result<crate::geo_update::Receipt> {
        let prepared = self.verify_prepared_geo(prepared, name).await?;
        tokio::task::spawn_blocking(move || prepared.publish())
            .await
            .context("Geo publication worker failed")?
    }

    #[cfg(unix)]
    pub(super) async fn publish_live_geo(
        &mut self,
        prepared: crate::geo_update::Prepared,
        name: &str,
    ) -> Result<crate::geo_update::Receipt> {
        let prepared = self.verify_prepared_geo(prepared, name).await?;
        if prepared.previous_sha256() == Some(prepared.candidate_sha256()) {
            return tokio::task::spawn_blocking(move || prepared.publish())
                .await
                .context("Geo no-change publication worker failed")?;
        }
        let data = self.options.data_dir.clone();
        crate::geo_live::begin(&data, name, prepared.previous_sha256(), prepared.candidate_sha256())?;
        self.retry_at = None;
        self.publish(CorePhase::Stopping, None);
        let result = async {
            self.stop_process().await?;
            let receipt = tokio::task::spawn_blocking(move || prepared.publish())
                .await
                .context("live Geo publication worker failed")??;
            ensure!(receipt.durable, "Geo publication directory sync failed");
            self.publish(CorePhase::Starting, None);
            self.start_inner().await?;
            tokio::select! {biased;
                _ = closing(&mut self.shutdown) => anyhow::bail!("Geo activation cancelled during shutdown"),
                _ = sleep(self.options.policy.probe_interval) => {},
            }
            self.observe_exit().await?;
            ensure!(
                self.status.borrow().phase == CorePhase::Running,
                "Geo candidate exited during activation health check"
            );
            timeout(self.options.policy.probe_timeout, self.client.get_version())
                .await
                .context("Geo activation health check timed out")??;
            let on_disk =
                crate::geo_validation::snapshot(&data, name)?.map(|bytes| crate::geo_validation::sha256(&bytes));
            ensure!(
                on_disk.as_deref() == Some(receipt.validation.sha256.as_str()),
                "Geo file changed during activation"
            );
            let commit = crate::geo_live::commit(&data)?;
            Ok::<_, anyhow::Error>((receipt, commit))
        }
        .await;
        match result {
            Ok((mut receipt, (durable, cleanup_pending))) => {
                receipt.durable &= durable;
                receipt.cleanup_pending |= cleanup_pending;
                self.begin_restoration(false).await;
                Ok(receipt)
            }
            Err(error) => {
                self.publish(CorePhase::Stopping, None);
                if let Err(stop) = self.stop_process().await {
                    self.publish(CorePhase::Failed, Some("Geo rollback requires core cleanup".into()));
                    return Err(error.context(format!("Geo candidate cleanup failed: {stop:#}")));
                }
                if let Err(recovery) = crate::geo_live::recover(&data) {
                    self.publish(CorePhase::Failed, Some("Geo rollback recovery required".into()));
                    return Err(error.context(format!("Geo rollback failed: {recovery:#}")));
                }
                self.publish(CorePhase::Stopped, None);
                if !*self.shutdown.borrow() {
                    self.publish(CorePhase::Starting, None);
                    if let Err(restart) = self.start_inner().await {
                        let _ = self.stop_process().await;
                        self.publish(
                            CorePhase::Failed,
                            Some("previous Geo restored but core restart failed".into()),
                        );
                        self.schedule_recovery();
                        return Err(error.context(format!("previous core restart failed: {restart:#}")));
                    }
                    self.begin_restoration(false).await;
                }
                Err(error.context("Geo activation failed; previous file restored"))
            }
        }
    }

    pub(super) async fn read_geo_settings(&self) -> Result<crate::geo_settings::Snapshot> {
        let (revision, config) = self.committed_config()?;
        let running = self.status.borrow().phase == CorePhase::Running;
        let actual = if running {
            timeout(Duration::from_secs(3), self.client.get_geo_config())
                .await
                .ok()
                .and_then(Result::ok)
        } else {
            None
        };
        crate::geo_settings::snapshot(
            &self.settings.runtime,
            config.as_ref(),
            revision,
            running,
            actual.as_ref(),
        )
    }

    pub(super) async fn read_resources(&self) -> Result<crate::resource_inventory::Inventory> {
        let revision = self.store.state().current.clone().map(|revision| revision.file);
        let config = if revision.is_some() {
            self.store.read_current()?
        } else {
            Mapping::new()
        };
        let running = self.status.borrow().phase == CorePhase::Running;
        let actual = if running {
            timeout(Duration::from_secs(3), self.client.get_geo_config())
                .await
                .ok()
                .and_then(Result::ok)
        } else {
            None
        };
        let geo_update = crate::resource_inventory::geo_update_policy_from_config(&config, running, actual.as_ref());
        let data = self.options.data_dir.clone();
        let bundle = self
            .options
            .resources
            .as_ref()
            .map(|resources| resources.directory().to_path_buf());
        tokio::task::spawn_blocking(move || {
            crate::resource_inventory::inspect(data, bundle, revision, config, geo_update)
        })
        .await
        .context("resource inventory worker failed")?
    }

    pub(super) async fn validate_geo(&self, name: String) -> Result<crate::geo_validation::Validation> {
        let data = self.options.data_dir.clone();
        tokio::task::spawn_blocking(move || crate::geo_validation::validate(&data, &name))
            .await
            .context("Geo validation worker failed")?
    }

    #[cfg(unix)]
    pub(super) async fn geo_seed_info(&self, name: String) -> Result<crate::geo_update::SeedInfo> {
        let resources = self
            .options
            .resources
            .clone()
            .context("Geo updates require bundle resources")?;
        let data = self.options.data_dir.clone();
        tokio::task::spawn_blocking(move || resources.geo_seed_info(&data, &name))
            .await
            .context("Geo seed worker failed")?
    }

    #[cfg(unix)]
    pub(super) async fn install_geo_seed(
        &mut self,
        request: crate::geo_update::InstallRequest,
    ) -> Result<crate::geo_update::Receipt> {
        ensure!(
            self.status.borrow().phase == CorePhase::Stopped && self.process.is_none(),
            "stop the core before installing a Geo seed"
        );
        let resources = self
            .options
            .resources
            .clone()
            .context("Geo updates require bundle resources")?;
        let data = self.options.data_dir.clone();
        if crate::dat_validation::DAT_FILES.contains(&request.name.as_str()) {
            let name = request.name.clone();
            let prepared = tokio::task::spawn_blocking(move || resources.prepare_dat_seed(&data, &request))
                .await
                .context("DAT staging worker failed")??;
            self.publish_prepared_geo(prepared, &name).await
        } else {
            tokio::task::spawn_blocking(move || resources.install_geo_seed(&data, &request))
                .await
                .context("Geo install worker failed")?
        }
    }

    #[cfg(unix)]
    pub(super) fn geo_online_info(&self, name: &str) -> Result<crate::geo_online::Info> {
        let config = self.store.read_current()?;
        crate::geo_online::info(&config, &self.options.data_dir, name)
    }

    /// Download from the committed source, then publish live (running) or in place (stopped).
    #[cfg(unix)]
    pub(super) async fn update_geo_online(
        &mut self,
        request: crate::geo_online::Request,
    ) -> Result<crate::geo_update::Receipt> {
        let running = self.status.borrow().phase == CorePhase::Running && self.process.is_some();
        ensure!(
            running || (self.status.borrow().phase == CorePhase::Stopped && self.process.is_none()),
            "Geo online update requires a running or stopped core"
        );
        let config = self.store.read_current()?;
        let (url, source_sha256) = crate::geo_online::source(&config, &request.name)?;
        ensure!(
            request.expected_source_sha256.eq_ignore_ascii_case(&source_sha256),
            "Geo source changed since inspection; inspect again"
        );
        let inspected = crate::geo_online::info(&config, &self.options.data_dir, &request.name)?;
        ensure!(
            match (&request.expected_current_sha256, &inspected.current_sha256) {
                (None, None) => true,
                (Some(expected), Some(actual)) => expected.eq_ignore_ascii_case(actual),
                _ => false,
            },
            "Geo file changed since online inspection; inspect again"
        );
        let route = self.geo_download_route(request.route, &config).await?;
        let downloaded = tokio::select! {
            biased;
            _ = closing(&mut self.shutdown) => bail!("Geo download cancelled during shutdown"),
            result = crate::geo_online::fetch(
                &url,
                &request.name,
                request.expected_download_sha256.as_deref(),
                &route,
                request.danger_accept_invalid_certs,
            ) => result?,
        };
        if running {
            self.observe_exit().await?;
            ensure!(
                self.status.borrow().phase == CorePhase::Running && self.process.is_some(),
                "core changed during Geo download; inspect again"
            );
        }
        let data = self.options.data_dir.clone();
        let name = request.name.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let install = crate::geo_update::InstallRequest {
                name: request.name,
                expected_current_sha256: request.expected_current_sha256,
                expected_seed_sha256: downloaded.seed.sha256.clone(),
                accept_metadata_only: request.accept_metadata_only,
            };
            crate::geo_update::prepare(&downloaded.directory, &data, &downloaded.seed, &install)
        })
        .await
        .context("online Geo staging worker failed")??;
        if running {
            self.publish_live_geo(prepared, &name).await
        } else {
            self.publish_prepared_geo(prepared, &name).await
        }
    }
}
