//! Managed core release discovery, staging, upgrade and activation.
use super::*;

impl CoreManager {
    /// Core release network work is independent of the lifecycle actor and subscriptions.
    pub async fn core_release(&self, version: Option<String>) -> Result<crate::core_release::CoreRelease> {
        self.core_release_for(version, crate::core_release::ReleaseChannel::Stable)
            .await
    }

    pub async fn alpha_core_release(&self, version: Option<String>) -> Result<crate::core_release::CoreRelease> {
        self.core_release_for(version, crate::core_release::ReleaseChannel::Alpha)
            .await
    }

    pub(super) async fn core_release_for(
        &self,
        version: Option<String>,
        channel: crate::core_release::ReleaseChannel,
    ) -> Result<crate::core_release::CoreRelease> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        let mut shutdown = self.shutdown.subscribe();
        tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("core release check cancelled during shutdown"),
            result = async { Ok(self.discover_core_release_for(version.as_deref(), channel).await?.release) } => result,
        }
    }

    pub async fn prepare_core_upgrade(&self, version: Option<String>) -> Result<crate::core_release::PreparedCore> {
        self.prepare_core_upgrade_for(version, crate::core_release::ReleaseChannel::Stable)
            .await
    }

    pub async fn prepare_alpha_core_upgrade(
        &self,
        version: Option<String>,
    ) -> Result<crate::core_release::PreparedCore> {
        self.prepare_core_upgrade_for(version, crate::core_release::ReleaseChannel::Alpha)
            .await
    }

    pub(super) async fn prepare_core_upgrade_for(
        &self,
        version: Option<String>,
        channel: crate::core_release::ReleaseChannel,
    ) -> Result<crate::core_release::PreparedCore> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let downloads = self
            .core_downloads
            .as_ref()
            .context("core preparation requires bundle-managed resources")?;
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        let mut shutdown = self.shutdown.subscribe();
        let cancellation = self.shutdown.subscribe();
        tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("core preparation cancelled during shutdown"),
            result = async { let resolved=self.discover_core_release_for(version.as_deref(), channel).await?; downloads.prepare_selected(resolved,&cancellation).await } => result,
        }
    }

    pub fn prepared_core_upgrade(&self, id: &str) -> Result<crate::core_release::PreparedCore> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        self.core_downloads
            .as_ref()
            .context("core preparation requires bundle-managed resources")?
            .inspect(id)
    }

    pub async fn stage_core_upgrade(&self, id: String) -> Result<crate::core_release::StagedCore> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let downloads = self
            .core_downloads
            .clone()
            .context("core staging requires bundle-managed resources")?;
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        self.call(
            |reply| CommandMessage::StageCoreUpgrade {
                id,
                downloads,
                reply,
                _permit,
            },
            "core manager stopped",
        )
        .await
    }

    pub fn staged_core_upgrade(&self, id: &str) -> Result<crate::core_release::StagedCore> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        self.core_downloads
            .as_ref()
            .context("core staging requires bundle-managed resources")?
            .inspect_stage(id)
    }

    pub async fn activate_core_upgrade(&self, id: String) -> Result<CoreActivation> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let downloads = self
            .core_downloads
            .clone()
            .context("core activation requires bundle-managed resources")?;
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        self.call(
            |reply| CommandMessage::ActivateCoreUpgrade {
                id,
                downloads,
                reply,
                _permit,
            },
            "core manager stopped",
        )
        .await
    }

    pub async fn core_installation(&self) -> Result<Option<crate::core_upgrade::CoreInstallation>> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(
            self.core_downloads.is_some(),
            "core installation requires bundle-managed resources"
        );
        self.call(CommandMessage::CoreInstallation, "core manager stopped")
            .await
    }

    pub async fn installed_core_version(&self) -> Result<String> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(
            self.core_downloads.is_some(),
            "core upgrade requires bundle-managed resources"
        );
        self.call(CommandMessage::InstalledCoreVersion, "core manager stopped")
            .await
    }

    pub async fn upgrade_clash_core(&self, force: bool) -> Result<CoreUpgradeReport> {
        self.upgrade_core_channel(force, crate::core_release::ReleaseChannel::Stable)
            .await
    }

    pub async fn upgrade_alpha_core(&self, force: bool) -> Result<CoreUpgradeReport> {
        self.upgrade_core_channel(force, crate::core_release::ReleaseChannel::Alpha)
            .await
    }

    pub(super) async fn upgrade_core_channel(
        &self,
        force: bool,
        channel: crate::core_release::ReleaseChannel,
    ) -> Result<CoreUpgradeReport> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let downloads = self
            .core_downloads
            .clone()
            .context("core upgrade requires bundle-managed resources")?;
        let permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        let mut shutdown = self.shutdown.subscribe();
        let release = tokio::select! {biased;
            _ = closing(&mut shutdown) => bail!("core upgrade cancelled during shutdown"),
            result = self.discover_core_release_for(None, channel) => result?,
        };
        let version = release.release.version.clone();
        if let Some(report) = self
            .call(
                |reply| CommandMessage::CheckCoreUpgrade { version, force, reply },
                "core manager stopped",
            )
            .await?
        {
            return Ok(report);
        }
        let cancellation = self.shutdown.subscribe();
        let prepared = tokio::select! {biased;
            _ = closing(&mut shutdown) => bail!("core upgrade cancelled during shutdown"),
            result = downloads.prepare_selected(release, &cancellation) => result?,
        };
        self.call(
            |reply| CommandMessage::UpgradePreparedCore {
                _permit: permit,
                prepared,
                force,
                downloads,
                reply,
            },
            "core manager stopped",
        )
        .await
    }

    pub(super) async fn core_download_routes(&self) -> Vec<crate::core_release::Route> {
        let mut routes = Vec::new();
        if self.status().phase == CorePhase::Running
            && let Ok(route) = self.managed_download_route().await
        {
            routes.push(route);
        }
        routes.extend([crate::core_release::Route::System, crate::core_release::Route::Direct]);
        routes
    }

    pub(super) async fn discover_core_release_for(
        &self,
        version: Option<&str>,
        channel: crate::core_release::ReleaseChannel,
    ) -> Result<crate::core_release::ResolvedRelease> {
        let routes = self.core_download_routes().await;
        let resolved = match channel {
            crate::core_release::ReleaseChannel::Stable => crate::core_release::discover_via(version, routes).await?,
            crate::core_release::ReleaseChannel::Alpha => {
                crate::core_release::discover_channel_via(version, routes, channel).await?
            }
        };
        self.logs.append(
            "core-upgrade",
            format!("release metadata resolved via {} route", resolved.route.name()),
        );
        Ok(resolved)
    }
}

impl Actor {
    pub(super) async fn recover_core_upgrade(&mut self) -> Result<()> {
        if self.options.resources.is_none() {
            return Ok(());
        }
        let core = self
            .options
            .binary
            .parent()
            .context("managed core directory missing")?
            .to_path_buf();
        if crate::core_upgrade::pending(&core)? {
            self.retry_at = None;
            self.publish(CorePhase::Stopping, None);
            self.stop_process().await?;
        }
        if let Some(was_running) = crate::core_upgrade::recover(&core)? {
            self.publish(CorePhase::Stopped, None);
            if was_running && !*self.shutdown.borrow() {
                self.publish(CorePhase::Starting, None);
                self.start_inner().await?;
                self.begin_restoration(false).await;
            }
        }
        Ok(())
    }

    pub(super) async fn installed_version(&mut self) -> Result<String> {
        if self.options.resources.is_some() {
            crate::core_upgrade::repairable(&self.options.binary)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let metadata = std::fs::symlink_metadata(&self.options.binary)?;
                if metadata.len() == 0 || metadata.permissions().mode() & 0o500 != 0o500 {
                    return Ok("unknown".into());
                }
            }
        }
        match crate::validation::probe_version(&self.options.binary, &mut self.shutdown, Duration::from_secs(5)).await {
            Ok(version) => Ok(version),
            Err(_) if self.options.resources.is_some() && !*self.shutdown.borrow() => {
                // An unreadable/broken core must not block the operation that repairs it.
                crate::core_upgrade::repairable(&self.options.binary)?;
                Ok("unknown".into())
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn check_upgrade(&mut self, version: &str, force: bool) -> Result<Option<CoreUpgradeReport>> {
        let from = self.installed_version().await?;
        Ok((!force && from == version).then(|| CoreUpgradeReport {
            upgraded: false,
            from,
            to: version.into(),
        }))
    }

    pub(super) async fn upgrade_prepared(
        &mut self,
        prepared: crate::core_release::PreparedCore,
        force: bool,
        downloads: Arc<crate::core_release::CoreDownloads>,
    ) -> Result<CoreUpgradeReport> {
        // Lifecycle/configuration commands may have run while the download was in flight.
        if let Some(report) = self.check_upgrade(&prepared.release.version, force).await? {
            return Ok(report);
        }
        let yaml = serde_yaml_ng::to_string(&read_config(&self.options.config).await?)?;
        let revision = self.store.state().current.map(|revision| revision.file);
        let stage = downloads
            .stage(&prepared.id, yaml, revision, &self.options.data_dir, &mut self.shutdown)
            .await?;
        let activated = self.activate_core(&stage.stage_id, downloads).await?;
        Ok(CoreUpgradeReport {
            upgraded: activated.upgraded,
            from: activated.from,
            to: activated.to,
        })
    }

    pub(super) async fn activate_core(
        &mut self,
        id: &str,
        downloads: Arc<crate::core_release::CoreDownloads>,
    ) -> Result<CoreActivation> {
        self.observe_exit().await?;
        let staged = downloads.inspect_stage(id)?;
        let yaml = serde_yaml_ng::to_string(&read_config(&self.options.config).await?)?;
        ensure!(
            crate::core_upgrade::config_hash(yaml.as_bytes()) == staged.config_sha256,
            "current configuration changed; stage the core again"
        );
        let revision = self.store.state().current.map(|revision| revision.file);
        let checked = downloads
            .stage(
                &staged.prepared.id,
                yaml,
                revision,
                &self.options.data_dir,
                &mut self.shutdown,
            )
            .await?;
        ensure!(checked.stage_id == id, "candidate configuration proof changed");
        let source = downloads.staged_binary(id)?;
        let from = self.installed_version().await?;
        let core = self
            .options
            .binary
            .parent()
            .context("managed core directory missing")?
            .to_path_buf();
        let was_running = self.status.borrow().phase == CorePhase::Running;
        let repair = from == "unknown";
        self.retry_at = None;
        let mut stopped = false;
        let result = async {
            let directory = core.clone();
            let candidate = checked.clone();
            let stop = self.shutdown.clone();
            let receipt = tokio::task::spawn_blocking(move || {
                ensure!(!*stop.borrow(), "core activation cancelled during shutdown");
                crate::core_upgrade::prepare_with_repair(&directory, &source, &candidate, was_running, repair)
            })
            .await
            .context("core replacement worker failed")??;
            ensure!(!*self.shutdown.borrow(), "core activation cancelled during shutdown");
            self.publish(CorePhase::Stopping, None);
            self.stop_process().await?;
            stopped = true;
            ensure!(!*self.shutdown.borrow(), "core activation cancelled during shutdown");
            crate::core_upgrade::publish(&core)?;
            self.publish(CorePhase::Starting, None);
            self.start_inner().await?;
            ensure!(
                self.status.borrow().version.as_deref() == Some(&receipt.version),
                "activated runtime version differs from candidate"
            );
            // A second live probe catches immediate exits after the first readiness response.
            tokio::select! {biased;
                _ = closing(&mut self.shutdown) => bail!("core activation cancelled during shutdown"),
                _ = sleep(self.options.policy.probe_interval) => {},
            }
            self.observe_exit().await?;
            ensure!(
                self.status.borrow().phase == CorePhase::Running,
                "candidate exited during activation health check"
            );
            let version = timeout(self.options.policy.probe_timeout, self.client.get_version())
                .await
                .context("activation health check timed out")??;
            ensure!(
                version.version == receipt.version,
                "activated core failed version health check"
            );
            self.verify_proxy_ports(&self.options.config.clone()).await?;
            if !was_running {
                self.stop_process().await?;
                self.publish(CorePhase::Stopped, None);
            }
            ensure!(!*self.shutdown.borrow(), "core activation cancelled during shutdown");
            crate::core_upgrade::commit(&core)?;
            crate::core_upgrade::recover(&core)?;
            Ok::<_, anyhow::Error>(receipt)
        }
        .await;
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                // A committed marker survives metadata/cleanup errors; never roll it back.
                if !crate::core_upgrade::pending(&core)? {
                    let committed = core.join(".core-upgrade/journal.json").try_exists()?;
                    if committed {
                        crate::core_upgrade::recover(&core).context("activated core metadata recovery required")?;
                        crate::core_upgrade::installation(&core)?.context("activated core receipt missing")?
                    } else {
                        // Preparation failed, or a committed cleanup failed after removing the log.
                        crate::core_upgrade::recover(&core)?;
                        if stopped && crate::core_upgrade::installation(&core)?.is_some_and(|r| r.stage_id == id) {
                            crate::core_upgrade::installation(&core)?.expect("checked receipt")
                        } else {
                            return Err(error);
                        }
                    }
                } else {
                    self.retry_at = None;
                    self.publish(CorePhase::Stopping, None);
                    if let Err(cleanup) = self.stop_process().await {
                        self.publish(
                            CorePhase::Failed,
                            Some("core upgrade rollback requires process cleanup".into()),
                        );
                        return Err(error.context(format!("core rollback cleanup failed: {cleanup:#}")));
                    }
                    if let Err(recovery) = crate::core_upgrade::recover(&core) {
                        self.publish(
                            CorePhase::Failed,
                            Some("core upgrade rollback recovery required".into()),
                        );
                        return Err(error.context(format!("core upgrade rollback recovery required: {recovery:#}")));
                    }
                    self.publish(CorePhase::Stopped, None);
                    if was_running && !*self.shutdown.borrow() {
                        self.publish(CorePhase::Starting, None);
                        if let Err(restart) = self.start_inner().await {
                            let _ = self.stop_process().await;
                            self.publish(
                                CorePhase::Failed,
                                Some("previous core restored but failed to restart".into()),
                            );
                            self.schedule_recovery();
                            return Err(error.context(format!("previous core restart failed: {restart:#}")));
                        }
                        self.begin_restoration(false).await;
                    }
                    let message = "core activation failed; previous core restored";
                    self.status.send_modify(|state| state.error = Some(message.into()));
                    bail!(message);
                }
            }
        };
        self.retry_at = None;
        if was_running {
            self.begin_restoration(false).await;
        }
        Ok(CoreActivation {
            upgraded: true,
            from,
            to: receipt.version.clone(),
            installation: receipt,
            status: self.status.borrow().clone(),
        })
    }

    pub(super) fn core_installation(&self) -> Result<Option<crate::core_upgrade::CoreInstallation>> {
        crate::core_upgrade::installation(self.options.binary.parent().expect("managed core directory"))
    }

    /// Stage against the committed configuration so the probe matches what will run.
    pub(super) async fn stage_core(
        &mut self,
        id: &str,
        downloads: &crate::core_release::CoreDownloads,
    ) -> Result<crate::core_release::StagedCore> {
        let yaml = serde_yaml_ng::to_string(&read_config(&self.options.config).await?)?;
        let revision = self.store.state().current.map(|revision| revision.file);
        downloads
            .stage(id, yaml, revision, &self.options.data_dir, &mut self.shutdown)
            .await
    }
}
