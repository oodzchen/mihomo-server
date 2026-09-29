//! Authoritative service settings and per-profile DNS decisions.
use super::*;

impl CoreManager {
    pub async fn connection_settings(&self) -> Result<crate::connection_settings::Snapshot> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            CommandMessage::ReadConnectionSettings,
            "Connection settings read cancelled during shutdown",
        )
        .await
    }

    pub async fn settings(&self) -> Result<ServiceSettings> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(CommandMessage::ReadSettings, "settings read cancelled during shutdown")
            .await
    }

    pub async fn profile_dns(&self, uid: String) -> Result<DnsOverrideState> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::ReadProfileDns { uid, reply },
            "DNS settings read cancelled during shutdown",
        )
        .await
    }

    pub async fn set_profile_dns(
        &self,
        uid: String,
        enabled: bool,
        confirmation: Option<String>,
    ) -> Result<DnsOverrideOutcome> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::SetProfileDns {
                uid,
                enabled,
                confirmation,
                reply,
            },
            "DNS settings update cancelled during shutdown",
        )
        .await
    }

    pub async fn set_settings(&self, runtime: RuntimeSettings) -> Result<ServiceSettings> {
        runtime.validate()?;
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::SetSettings {
                runtime: Box::new(runtime),
                reply,
            },
            "settings update cancelled during shutdown",
        )
        .await
    }
}

impl Actor {
    pub(super) async fn update_settings(&mut self, runtime: RuntimeSettings) -> Result<ServiceSettings> {
        let candidate = ServiceSettings {
            schema_version: 1,
            runtime,
            profile_dns: self.settings.profile_dns.clone(),
        };
        candidate.validate()?;
        if candidate == self.settings {
            return Ok(self.settings.clone());
        }
        self.observe_exit().await?;
        let result = async {
            if self.store.state().current.is_none() {
                ensure!(!*self.shutdown.borrow(), "settings update cancelled during shutdown");
                self.settings_store.replace(candidate.clone())?;
                return Ok(());
            }
            self.settings = candidate.clone();
            let active = self.store.state().active_profile;
            let config = if let Some(uid) = &active {
                let plan = self.profile_store.read_generation(uid)?;
                self.finish_generation(plan).await?
            } else {
                self.store.read_current()?.into()
            };
            self.apply_with_change(config, active, Some(ProfileChange::Settings(Box::new(candidate))))
                .await
        }
        .await;
        self.settings = self.settings_store.snapshot();
        result?;
        Ok(self.settings.clone())
    }

    pub(super) fn dns_decision(&self, uid: &str, source: Option<String>) -> DnsOverrideState {
        let requested = self.settings.profile_dns.get(uid).map_or(
            self.settings.runtime.dns.is_some() || self.settings.runtime.hosts.is_some(),
            |settings| settings.enabled,
        );
        DnsOverrideState::new(
            uid,
            source,
            requested,
            self.dns_confirmations.get(uid).map(String::as_str),
        )
    }

    pub(super) fn dns_runtime(&self, state: &DnsOverrideState) -> RuntimeSettings {
        let mut runtime = self.settings.runtime.clone();
        if !state.enabled {
            runtime.dns = None;
            runtime.hosts = None;
        }
        runtime
    }

    pub(super) async fn update_profile_dns(
        &mut self,
        uid: &str,
        enabled: bool,
        confirmation: Option<String>,
    ) -> Result<DnsOverrideOutcome> {
        ensure!(
            self.store.state().active_profile.as_deref() == Some(uid),
            "active profile changed; retry DNS settings"
        );
        let source = self.profile_store.dns_source(uid)?;
        if enabled && let Some(source) = source.as_ref().filter(|s| Some(s.as_str()) != confirmation.as_deref()) {
            return Ok(DnsOverrideOutcome::ConfirmationRequired { source: source.clone() });
        }
        ensure!(
            !enabled || self.settings.runtime.dns.is_some() || self.settings.runtime.hosts.is_some(),
            "save DNS or hosts settings before enabling the profile override"
        );
        self.observe_exit().await?;
        let old_revision = self.store.state().current;
        let old_confirmation = self.dns_confirmations.remove(uid);
        if enabled && let Some(source) = source {
            self.dns_confirmations.insert(uid.into(), source);
        }
        self.settings
            .profile_dns
            .insert(uid.into(), ProfileDnsSettings { enabled });
        let result = async {
            let generation = self.profile_store.read_generation(uid)?;
            let config = self.finish_generation(generation).await?;
            self.apply_with_change(
                config,
                Some(uid.into()),
                Some(ProfileChange::Settings(Box::new(self.settings.clone()))),
            )
            .await
        }
        .await;
        self.settings = self.settings_store.snapshot();
        if self.store.state().current == old_revision {
            self.dns_confirmations.remove(uid);
            if let Some(source) = old_confirmation {
                self.dns_confirmations.insert(uid.into(), source);
            }
        }
        result?;
        Ok(DnsOverrideOutcome::Applied {
            state: self.dns_decision(uid, self.profile_store.dns_source(uid)?),
        })
    }

    pub(super) fn enforce_runtime_settings(&self, config: Mapping, runtime: &RuntimeSettings) -> Result<Mapping> {
        let enforced = runtime.enforce(config.clone())?;
        for field in runtime.overridden_fields(&config, &enforced)? {
            self.logs.append(
                "settings",
                format!("{field} is managed by settings; override discarded"),
            );
        }
        Ok(enforced)
    }

    pub(super) fn read_profile_dns(&self, uid: &str) -> Result<DnsOverrideState> {
        self.profile_store
            .dns_source(uid)
            .map(|source| self.dns_decision(uid, source))
    }

    /// Committed revision and its configuration, when one exists.
    pub(super) fn committed_config(&self) -> Result<(Option<String>, Option<Mapping>)> {
        let revision = self.store.state().current.map(|revision| revision.file);
        let config = if revision.is_some() {
            Some(self.store.read_current()?)
        } else {
            None
        };
        Ok((revision, config))
    }

    pub(super) async fn read_connection_settings(&self) -> Result<crate::connection_settings::Snapshot> {
        let (revision, config) = self.committed_config()?;
        let running = self.status.borrow().phase == CorePhase::Running;
        let actual = if running {
            timeout(Duration::from_secs(3), self.client.get_connection_config())
                .await
                .ok()
                .and_then(Result::ok)
        } else {
            None
        };
        crate::connection_settings::snapshot(
            &self.settings.runtime,
            config.as_ref(),
            revision,
            running,
            actual.as_ref(),
        )
    }
}
