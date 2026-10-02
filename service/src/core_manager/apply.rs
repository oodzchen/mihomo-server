//! Configuration staging and transactional application with rollback.
use super::*;

impl Actor {
    pub(super) async fn stage(
        &mut self,
        config: ConfigCandidate,
        active_profile: Option<String>,
    ) -> Result<(PathBuf, Option<DnsOverrideState>)> {
        let (config, runtime, dns) = match config {
            ConfigCandidate::Raw(config) => {
                let dns = active_profile
                    .as_deref()
                    .map(|uid| {
                        self.profile_store
                            .dns_source(uid)
                            .map(|source| self.dns_decision(uid, source))
                    })
                    .transpose()?;
                let runtime = dns
                    .as_ref()
                    .map_or_else(|| self.settings.runtime.clone(), |state| self.dns_runtime(state));
                (runtime.prepare(config)?, runtime, dns)
            }
            ConfigCandidate::Enhanced { config, runtime, dns } => (config, *runtime, Some(*dns)),
        };
        let config = self.enforce_runtime_settings(config, &runtime)?;
        let config = headless_core::enhance::finalize::finalize(config);
        let config = self.isolate(config, &runtime);
        validate_resource_declarations(&config)?;
        let config = headless_core::config::resource_paths::prepare_owned(
            config,
            &self.options.data_dir,
            &crate::validation::protected_paths(&self.options.data_dir, &self.options.config, &self.options.binary),
        )?;
        let revision = self.store.stage(config)?;
        let path = self.store.path(&revision)?;
        crate::validation::validate(
            &self.options.binary,
            &self.options.data_dir,
            &path,
            &mut self.shutdown,
            self.options.policy.validation_timeout,
        )
        .await?;
        ensure!(
            !*self.shutdown.borrow(),
            "configuration application cancelled during shutdown"
        );
        let previous = self.store.state();
        if let Err(error) = self.store.begin_profile(revision, active_profile) {
            let rollback = self.store.restore(previous);
            return Err(error.context(format!("candidate journal failed; recovery: {rollback:?}")));
        }
        Ok((path, dns))
    }

    pub(super) async fn apply(
        &mut self,
        config: impl Into<ConfigCandidate>,
        active_profile: Option<String>,
    ) -> Result<()> {
        self.apply_with_change(config, active_profile, None).await
    }

    pub(super) async fn apply_with_change(
        &mut self,
        config: impl Into<ConfigCandidate>,
        active_profile: Option<String>,
        change: Option<ProfileChange>,
    ) -> Result<()> {
        let previous = self.store.state();
        let old_path = self.options.config.clone();
        let was_running = self.status.borrow().phase == CorePhase::Running;
        let (candidate, dns) = self.stage(config.into(), active_profile.clone()).await?;
        let refreshing = matches!(&change, Some(ProfileChange::Refresh(_) | ProfileChange::RawEdit { .. }));
        let merging = matches!(&change, Some(ProfileChange::Enhancement(_)));
        let mut settings_candidate = match &change {
            Some(ProfileChange::Settings(settings)) => (**settings).clone(),
            _ => self.settings.clone(),
        };
        if let Some(state) = &dns
            && (settings_candidate.runtime.dns.is_some()
                || settings_candidate.runtime.hosts.is_some()
                || settings_candidate.profile_dns.contains_key(&state.uid))
        {
            settings_candidate
                .profile_dns
                .insert(state.uid.clone(), ProfileDnsSettings { enabled: state.enabled });
        }
        let setting = settings_candidate != self.settings_store.snapshot();
        // A valid manual apply cancels automatic recovery; it remains stopped if stopped.
        self.retry_at = None;
        self.cancel_restoration();
        let mut live_attempted = false;
        let result = async {
            if was_running {
                let config = read_config(&candidate).await?;
                self.tun_preflight(&config)?;
            }
            if setting {
                self.settings_store.begin(settings_candidate, self.store.state().pending.context("settings candidate revision missing")?)?;
            }
            match change {
                Some(ProfileChange::Settings(_)) => {},
                Some(ProfileChange::RawEdit { revision, yaml }) => self.profile_store.begin_raw_edit(
                    active_profile.as_deref().context("raw profile UID missing")?, &revision, &yaml, self.store.state().pending,
                )?,
                Some(ProfileChange::Refresh(remote)) => self.profile_store.begin_refresh(
                    active_profile.as_deref().context("refresh profile UID missing")?,
                    *remote,
                    self.store.state().pending,
                )?,
                Some(ProfileChange::Enhancement(plan)) => self
                    .profile_store
                    .begin_enhancement(*plan, self.store.state().pending)?,
                None => {}
            }
            if was_running {
                live_attempted = true;
                if let Err(error) = self.reload(&candidate).await {
                    ensure!(
                        !*self.shutdown.borrow(),
                        "application cancelled during shutdown: {error:#}"
                    );
                    self.logs.append(
                        "manager",
                        format!("reload failed; restarting with candidate: {error:#}"),
                    );
                    self.publish(CorePhase::Stopping, None);
                    self.stop_process().await?;
                    self.options.config = candidate.clone();
                    self.start_core().await?;
                }
            } else {
                self.options.config = candidate;
            }
            ensure!(
                !*self.shutdown.borrow(),
                "configuration commit cancelled during shutdown"
            );
            if refreshing {
                self.profile_store.publish_refresh()?;
            }
            if merging {
                self.profile_store.publish_enhancement()?;
            }
            if setting {
                self.settings_store.publish()?;
            }
            self.profile_store.set_current(active_profile.as_deref())?;
            self.store.commit()?;
            self.settings = self.settings_store.snapshot();
            if let Some(state) = &dns {
                if self.dns_confirmations.get(&state.uid).map(String::as_str) != state.source.as_deref() || !state.enabled {
                    self.dns_confirmations.remove(&state.uid);
                }
                if state.requested && !state.enabled {
                    self.logs.append("settings", format!("DNS override auto-disabled for profile {}: provider DNS requires current-session confirmation", state.uid));
                }
            }
            self.profile_state.send_replace(self.profile_store.snapshot());
            self.status.send_modify(|state| {
                state.config_revision = self.store.state().current.map(|revision| revision.file);
                state.active_profile = self.store.state().active_profile;
                state.error = None;
                state.recovery_attempt = 0;
                if !was_running {
                    state.phase = CorePhase::Stopped;
                }
            });
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            self.options.config = old_path;
            let disk = self.store.restore(previous);
            let settings_rollback = self.settings_store.recover(self.store.state().current.as_ref());
            self.settings = self.settings_store.snapshot();
            let refresh_rollback = self
                .profile_store
                .recover_refresh(self.store.state().current.as_ref())
                .and_then(|()| {
                    self.profile_store
                        .recover_enhancement(self.store.state().current.as_ref())
                });
            let profile_rollback = self
                .profile_store
                .set_current(self.store.state().active_profile.as_deref());
            self.profile_state.send_replace(self.profile_store.snapshot());
            let recovery = if was_running && live_attempted && !*self.shutdown.borrow() {
                self.publish(CorePhase::Stopping, None);
                match self.stop_process().await {
                    Ok(()) => self.start_core().await,
                    Err(error) => Err(error),
                }
            } else {
                Ok(())
            };
            let mut message = format!("configuration application failed: {error:#}");
            if let Err(error) = settings_rollback {
                message.push_str(&format!("; settings rollback failed: {error:#}"));
            }
            if let Err(error) = refresh_rollback {
                message.push_str(&format!("; profile change rollback failed: {error:#}"));
            }
            if let Err(error) = profile_rollback {
                message.push_str(&format!("; profile rollback failed: {error:#}"));
            }
            if let Err(error) = disk {
                message.push_str(&format!("; disk rollback failed: {error:#}"));
            }
            if let Err(error) = recovery {
                message.push_str(&format!("; core recovery failed: {error:#}"));
            }
            bail!(message);
        }
        // Cleanup errors remain observable without undoing an already committed profile change.
        self.profile_store
            .recover_refresh(self.store.state().current.as_ref())?;
        self.profile_store
            .recover_enhancement(self.store.state().current.as_ref())?;
        if setting {
            self.settings_store.recover(self.store.state().current.as_ref())?;
        }
        if was_running {
            self.begin_restoration(true).await;
        }
        Ok(())
    }
}
