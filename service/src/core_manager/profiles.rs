//! Subscription profiles: imports, raw edits, enhancements and remote refresh.
use super::*;

impl CoreManager {
    pub fn profiles(&self) -> IProfiles {
        self.profiles.borrow().clone()
    }

    pub fn subscribe_profiles(&self) -> watch::Receiver<IProfiles> {
        self.profiles.clone()
    }

    pub async fn import_profile(&self, path: PathBuf, name: Option<String>) -> Result<PrfItem> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::ImportProfile { path, name, reply },
            "profile import cancelled during shutdown",
        )
        .await
    }

    pub async fn profile_raw(&self, uid: String) -> Result<RawContent> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        self.call(
            |reply| CommandMessage::ReadProfileRaw { uid, reply },
            "profile raw read cancelled during shutdown",
        )
        .await
    }

    pub async fn set_profile_raw(&self, uid: String, revision: String, yaml: String) -> Result<RawContent> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(
            uid.len() <= 256 && revision.len() <= 256,
            "profile UID/revision exceeds 256 bytes"
        );
        ensure!(yaml.len() <= runtime::MAX_CONFIG_BYTES, "profile exceeds 8 MiB");
        self.call(
            |reply| CommandMessage::SetProfileRaw {
                uid,
                revision,
                yaml,
                reply,
            },
            "profile raw update cancelled during shutdown",
        )
        .await
    }

    pub async fn profile_merge(&self, uid: String) -> Result<EnhancementContent> {
        self.read_enhancement(uid, ProfileEnhancement::Merge).await
    }

    pub async fn global_merge(&self) -> Result<EnhancementContent> {
        self.read_enhancement("Merge".into(), ProfileEnhancement::Global(GlobalEnhancementKind::Merge))
            .await
    }

    pub async fn global_script(&self) -> Result<ScriptContent> {
        let content = self
            .read_enhancement(
                "Script".into(),
                ProfileEnhancement::Global(GlobalEnhancementKind::Script),
            )
            .await?;
        Ok(ScriptContent {
            uid: content.uid,
            source: content.yaml,
        })
    }

    pub async fn set_global_merge(&self, yaml: Option<String>) -> Result<PrfItem> {
        self.update_enhancement(
            "Merge".into(),
            ProfileEnhancement::Global(GlobalEnhancementKind::Merge),
            yaml,
        )
        .await
    }

    pub async fn set_global_script(&self, source: Option<String>) -> Result<PrfItem> {
        if let Some(source) = &source {
            headless_core::enhance::script::validate_source(source)?;
        }
        self.update_enhancement(
            "Script".into(),
            ProfileEnhancement::Global(GlobalEnhancementKind::Script),
            source,
        )
        .await
    }

    pub async fn profile_sequence(&self, uid: String, kind: SequenceKind) -> Result<EnhancementContent> {
        self.read_enhancement(uid, ProfileEnhancement::Sequence(kind)).await
    }

    pub async fn profile_script(&self, uid: String) -> Result<ScriptContent> {
        let content = self.read_enhancement(uid, ProfileEnhancement::Script).await?;
        Ok(ScriptContent {
            uid: content.uid,
            source: content.yaml,
        })
    }

    pub async fn set_profile_script(&self, uid: String, source: Option<String>) -> Result<PrfItem> {
        if let Some(source) = &source {
            headless_core::enhance::script::validate_source(source)?;
        }
        self.update_enhancement(uid, ProfileEnhancement::Script, source).await
    }

    pub(super) async fn read_enhancement(&self, uid: String, kind: ProfileEnhancement) -> Result<EnhancementContent> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        self.call(
            |reply| CommandMessage::ReadEnhancement { uid, kind, reply },
            "enhancement read cancelled during shutdown",
        )
        .await
    }

    pub async fn set_profile_merge(&self, uid: String, yaml: Option<String>) -> Result<PrfItem> {
        self.update_enhancement(uid, ProfileEnhancement::Merge, yaml).await
    }

    pub async fn set_profile_sequence(&self, uid: String, kind: SequenceKind, yaml: Option<String>) -> Result<PrfItem> {
        self.update_enhancement(uid, ProfileEnhancement::Sequence(kind), yaml)
            .await
    }

    pub(super) async fn update_enhancement(
        &self,
        uid: String,
        kind: ProfileEnhancement,
        yaml: Option<String>,
    ) -> Result<PrfItem> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        ensure!(
            yaml.as_ref().is_none_or(|yaml| yaml.len() <= runtime::MAX_CONFIG_BYTES),
            "enhancement exceeds 8 MiB"
        );
        self.call(
            |reply| CommandMessage::SetEnhancement { uid, kind, yaml, reply },
            "enhancement update cancelled during shutdown",
        )
        .await
    }

    pub async fn edit_profile(&self, uid: String, patch: ProfilePatch) -> Result<PrfItem> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        self.call(
            |reply| CommandMessage::EditProfile { uid, patch, reply },
            "profile edit cancelled during shutdown",
        )
        .await
    }

    pub async fn delete_profile(&self, uid: String) -> Result<IProfiles> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        self.call(
            |reply| CommandMessage::DeleteProfile { uid, reply },
            "profile deletion cancelled during shutdown",
        )
        .await
    }

    pub async fn select_profile(&self, uid: String) -> Result<CoreStatus> {
        self.request(Operation::SelectProfile(uid)).await
    }

    /// Upload content without exposing server filesystem paths to clients.
    pub async fn import_profile_yaml(&self, yaml: String, name: String) -> Result<PrfItem> {
        ensure!(yaml.len() <= runtime::MAX_CONFIG_BYTES, "profile exceeds 8 MiB");
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::ImportProfileYaml { yaml, name, reply },
            "profile import cancelled during shutdown",
        )
        .await
    }

    /// Network work cannot block child supervision; only the completed import mutates storage.
    pub async fn import_remote_profile(
        &self,
        url: String,
        name: Option<String>,
        options: crate::remote::RemoteOptions,
    ) -> Result<PrfItem> {
        ensure!(url.len() <= 8192, "subscription URL exceeds 8 KiB");
        ensure!(
            name.as_ref().is_none_or(|name| name.len() <= 256),
            "profile name exceeds 256 bytes"
        );
        ensure!(
            options.user_agent.as_ref().is_none_or(|agent| agent.len() <= 1024),
            "subscription user agent exceeds 1 KiB"
        );
        let mut shutdown = self.shutdown.subscribe();
        let _permit = tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("service is shutting down"),
            permit = Arc::clone(&self.remote_admission).acquire_owned() => permit?,
        };
        let profile = tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("remote import cancelled during shutdown"),
            result = self.download_remote(&url, name.as_deref(), options) => result?,
        };
        let (reply, result) = oneshot::channel();
        tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("remote import cancelled during shutdown"),
            sent = self.commands.send(CommandMessage::ImportRemote { profile: Box::new(profile), reply }) => sent.context("core manager stopped")?,
        }
        result.await.context("remote import cancelled during shutdown")?
    }

    /// The source file guards against a second download overwriting a newer refresh.
    pub async fn refresh_profile(&self, uid: String) -> Result<PrfItem> {
        self.refresh_profile_mode(uid, false).await
    }

    pub(super) async fn refresh_profile_mode(&self, uid: String, automatic: bool) -> Result<PrfItem> {
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        let source = self
            .profiles
            .borrow()
            .items
            .iter()
            .flatten()
            .find(|item| item.uid.as_deref() == Some(uid.as_str()))
            .cloned()
            .context("remote profile not found")?;
        ensure!(
            source.itype.as_deref() == Some("remote"),
            "only remote profiles can be refreshed"
        );
        if automatic {
            ensure!(scheduler::eligible(&source), "automatic subscription update disabled");
        }
        let options = crate::remote::RemoteOptions::from_profile(source.option.as_ref())?;
        let url = source.url.as_deref().context("remote URL missing")?;
        let mut shutdown = self.shutdown.subscribe();
        let _permit = tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("service is shutting down"),
            permit = Arc::clone(&self.remote_admission).acquire_owned() => permit?,
        };
        if automatic {
            let snapshot = self.profiles();
            let current = snapshot
                .items
                .iter()
                .flatten()
                .find(|item| item.uid.as_deref() == Some(uid.as_str()))
                .context("scheduled profile removed while waiting")?;
            ensure!(
                scheduler::eligible(current)
                    && current.file == source.file
                    && current.url == source.url
                    && current.option == source.option,
                "scheduled profile changed while waiting for admission"
            );
        }
        let profile = tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("remote refresh cancelled during shutdown"),
            result = self.download_remote(url, None, options) => result?,
        };
        let (reply, result) = oneshot::channel();
        tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("remote refresh cancelled during shutdown"),
            sent = self.commands.send(CommandMessage::RefreshRemote {
                automatic, source: Box::new(source), profile: Box::new(profile), reply,
            }) => sent.context("core manager stopped")?,
        }
        result.await.context("remote refresh cancelled during shutdown")?
    }

    /// Resolve only from the current child's reported ports and committed authentication.
    pub(super) async fn managed_download_route(&self) -> Result<crate::core_release::Route> {
        let mut state = self.state.clone();
        let snapshot = state.borrow_and_update().clone();
        ensure!(
            snapshot.phase == CorePhase::Running,
            "self_proxy requires a running managed core"
        );
        let route = timeout(Duration::from_secs(3), async {
            let runtime = self.runtime_config().await?;
            let core = self.client.get_base_config().await?;
            ensure!(
                same_proxy_snapshot(&snapshot, &self.status()),
                "managed proxy changed during route resolution; retry"
            );
            crate::proxy_access::verify_ports(&runtime, &core)?;
            crate::remote::ManagedProxy::from_core(&core, &runtime)
        })
        .await
        .context("managed proxy query timed out")??;
        Ok(crate::core_release::Route::Managed {
            proxy: route,
            snapshot: Box::new(snapshot),
            state,
        })
    }

    /// Resolve after admission, then cancel if the child or committed configuration changes.
    pub(super) async fn download_remote(
        &self,
        url: &str,
        name: Option<&str>,
        options: crate::remote::RemoteOptions,
    ) -> Result<headless_core::config::remote::RemoteProfile> {
        if options.self_proxy != Some(true) {
            return crate::remote::download(url, name, options).await;
        }
        let route = self.managed_download_route().await?;
        let crate::core_release::Route::Managed { proxy, .. } = &route else {
            unreachable!()
        };
        route
            .run(crate::remote::download_via(url, name, options, Some(proxy.clone())))
            .await
    }
}

impl Actor {
    pub(super) async fn import_profile(&mut self, path: &Path, name: Option<String>) -> Result<PrfItem> {
        ensure!(!*self.shutdown.borrow(), "profile import cancelled during shutdown");
        ensure!(
            tokio::fs::metadata(path).await?.len() <= runtime::MAX_CONFIG_BYTES as u64,
            "profile exceeds 8 MiB"
        );
        let yaml = tokio::fs::read_to_string(path).await?;
        ensure!(!*self.shutdown.borrow(), "profile import cancelled during shutdown");
        let name = name.unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Local File")
                .to_owned()
        });
        self.profile_store.import_local_with_defaults(&name, &yaml)
    }

    pub(super) async fn set_enhancement(
        &mut self,
        uid: &str,
        kind: ProfileEnhancement,
        yaml: Option<String>,
    ) -> Result<PrfItem> {
        self.observe_exit().await?;
        if let ProfileEnhancement::Global(kind) = kind {
            return self.set_global_enhancement(kind, yaml).await;
        }
        if yaml.is_none() {
            let content = match kind {
                ProfileEnhancement::Global(_) => unreachable!(),
                ProfileEnhancement::Sequence(kind) => self.profile_store.read_sequence(uid, kind)?,
                ProfileEnhancement::Merge => self.profile_store.read_merge(uid)?,
                ProfileEnhancement::Script => {
                    let content = self.profile_store.read_script(uid)?;
                    EnhancementContent {
                        uid: content.uid,
                        yaml: content.source,
                    }
                }
            };
            if content.uid.is_none() {
                return Ok(self.profile_store.get_item(uid)?.clone());
            }
        }
        let mut plan = match kind {
            ProfileEnhancement::Global(_) => unreachable!(),
            ProfileEnhancement::Sequence(kind) => self.profile_store.prepare_sequence(uid, kind, yaml)?,
            ProfileEnhancement::Merge => self.profile_store.prepare_merge(uid, yaml)?,
            ProfileEnhancement::Script => self.profile_store.prepare_script(uid, yaml)?,
        };
        let config = self
            .finish_generation(plan.generation.take().context("linked generation missing")?)
            .await?;
        if self.store.state().active_profile.as_deref() == Some(uid) {
            self.apply_with_change(
                config,
                Some(uid.into()),
                Some(ProfileChange::Enhancement(Box::new(plan))),
            )
            .await?;
        } else {
            self.validate_inactive_candidate(&config).await?;
            let result = (|| {
                self.profile_store.begin_enhancement(plan, None)?;
                self.profile_store.publish_enhancement()
            })();
            let recovery = self
                .profile_store
                .recover_enhancement(self.store.state().current.as_ref());
            result.map_err(|error| error.context(format!("enhancement recovery: {recovery:?}")))?;
            recovery?;
        }
        Ok(self.profile_store.get_item(uid)?.clone())
    }

    pub(super) async fn validate_inactive_candidate(&mut self, candidate: &ConfigCandidate) -> Result<()> {
        let (config, runtime) = match candidate {
            ConfigCandidate::Raw(config) => (config.clone(), &self.settings.runtime),
            ConfigCandidate::Enhanced { config, runtime, .. } => (config.clone(), &**runtime),
        };
        let config = headless_core::enhance::finalize::finalize(config);
        let config = match &self.options.isolation {
            Some(isolation) => isolation.apply(config, runtime).0,
            None => config,
        };
        validate_resource_declarations(&config)?;
        let validation_config = headless_core::config::resource_paths::prepare_owned(
            config,
            &self.options.data_dir,
            &crate::validation::protected_paths(&self.options.data_dir, &self.options.config, &self.options.binary),
        )?;
        let validation = self.store.stage(validation_config)?;
        crate::validation::validate(
            &self.options.binary,
            &self.options.data_dir,
            &self.store.path(&validation)?,
            &mut self.shutdown,
            self.options.policy.validation_timeout,
        )
        .await
    }

    pub(super) async fn set_global_enhancement(
        &mut self,
        kind: GlobalEnhancementKind,
        source: Option<String>,
    ) -> Result<PrfItem> {
        let source = source.unwrap_or_else(|| kind.default_source().into());
        let active = self.store.state().active_profile;
        let mut plan = self
            .profile_store
            .prepare_global(kind, source.clone(), active.as_deref())?;
        if let Some(generation) = plan.generation.take() {
            let config = self.finish_generation(generation).await?;
            self.apply_with_change(config, active, Some(ProfileChange::Enhancement(Box::new(plan))))
                .await?;
        } else {
            if kind == GlobalEnhancementKind::Script && source != DEFAULT_GLOBAL_SCRIPT {
                self.check_script(source).await?;
            } else if kind == GlobalEnhancementKind::Merge {
                let merge_map = runtime::parse_profile(&source)?;
                validate_resource_declarations(&merge_map)?;
            }
            ensure!(!*self.shutdown.borrow(), "global edit cancelled during shutdown");
            let result = (|| {
                self.profile_store.begin_enhancement(plan, None)?;
                self.profile_store.publish_enhancement()
            })();
            let recovery = self
                .profile_store
                .recover_enhancement(self.store.state().current.as_ref());
            result.map_err(|error| error.context(format!("global enhancement recovery: {recovery:?}")))?;
            recovery?;
        }
        Ok(self.profile_store.get_item(kind.uid())?.clone())
    }

    pub(super) async fn check_script(&mut self, source: String) -> Result<()> {
        let binary = self
            .options
            .script_worker
            .clone()
            .map_or_else(std::env::current_exe, Ok)?;
        let response = crate::script::execute(
            &binary,
            headless_core::enhance::script::ScriptRequest {
                source,
                config: Mapping::new(),
                name: String::new(),
                check_only: true,
            },
            &mut self.shutdown,
            self.options.policy.script_timeout,
        )
        .await?;
        if let Some(error) = response.error {
            bail!("{error}");
        }
        ensure!(response.config.is_some(), "script syntax check returned no result");
        Ok(())
    }

    pub(super) async fn finish_generation(&mut self, generation: GenerationPlan) -> Result<ConfigCandidate> {
        let dns = self.dns_decision(&generation.profile_uid, generation.dns_source);
        let runtime = self.dns_runtime(&dns);
        let config = runtime.prepare(generation.config)?;
        let config = runtime::generate(config, &generation.global_merge)?;
        let config = self
            .finish_script(config, generation.global_script, generation.name.clone())
            .await?;
        let config = runtime::generate(config, &generation.profile_merge)?;
        let config = self.finish_script(config, generation.script, generation.name).await?;
        Ok(ConfigCandidate::Enhanced {
            config: self.enforce_runtime_settings(config, &runtime)?,
            runtime: Box::new(runtime),
            dns: Box::new(dns),
        })
    }

    pub(super) async fn finish_script(
        &mut self,
        config: Mapping,
        source: Option<String>,
        name: String,
    ) -> Result<Mapping> {
        let Some(source) = source else {
            return Ok(config);
        };
        // The exact upstream identity template has no side effects or changes.
        // Avoid spawning workers for untouched defaults, including on non-Linux.
        if source == DEFAULT_GLOBAL_SCRIPT {
            return runtime::generate(config, &Mapping::new());
        }
        let binary = match &self.options.script_worker {
            Some(binary) => binary.clone(),
            None => std::env::current_exe()?,
        };
        let response = crate::script::execute(
            &binary,
            headless_core::enhance::script::ScriptRequest {
                source,
                config,
                name,
                check_only: false,
            },
            &mut self.shutdown,
            self.options.policy.script_timeout,
        )
        .await?;
        for (level, text) in response.logs {
            self.logs.append("script", format!("[{level}] {text}"));
        }
        if let Some(error) = response.error {
            bail!("{error}");
        }
        runtime::generate(
            response.config.context("script returned no configuration")?,
            &Mapping::new(),
        )
    }

    pub(super) async fn update_profile_raw(&mut self, uid: &str, revision: String, yaml: String) -> Result<RawContent> {
        self.observe_exit().await?;
        let previous = self.profile_store.read_raw(uid)?;
        ensure!(
            previous.revision == revision,
            "profile raw revision changed; reload before saving"
        );
        let raw = runtime::parse_profile(&yaml)?;
        validate_resource_declarations(&raw)?;
        // Upstream first validates original YAML even for an inactive profile.
        // Normalize only its probe copy; the submitted source is preserved.
        // An immutable validation revision changes no runtime manifest or catalog.
        let validation_config = headless_core::config::resource_paths::prepare_owned(
            raw.clone(),
            &self.options.data_dir,
            &crate::validation::protected_paths(&self.options.data_dir, &self.options.config, &self.options.binary),
        )?;
        let validation = self.store.stage(validation_config)?;
        crate::validation::validate(
            &self.options.binary,
            &self.options.data_dir,
            &self.store.path(&validation)?,
            &mut self.shutdown,
            self.options.policy.validation_timeout,
        )
        .await?;
        if self.store.state().active_profile.as_deref() == Some(uid) {
            let generation = self.profile_store.generation(uid, raw)?;
            let config = self.finish_generation(generation).await?;
            self.apply_with_change(
                config,
                Some(uid.into()),
                Some(ProfileChange::RawEdit { revision, yaml }),
            )
            .await?;
        } else {
            let result = (|| {
                self.profile_store.begin_raw_edit(uid, &revision, &yaml, None)?;
                self.profile_store.publish_refresh()
            })();
            let recovery = self.profile_store.recover_refresh(self.store.state().current.as_ref());
            result.map_err(|error| error.context(format!("raw edit recovery: {recovery:?}")))?;
            recovery?;
        }
        self.profile_store.read_raw(uid)
    }

    pub(super) async fn refresh_remote(
        &mut self,
        source: PrfItem,
        remote: headless_core::config::remote::RemoteProfile,
    ) -> Result<PrfItem> {
        let uid = source.uid.as_deref().context("remote UID missing")?.to_string();
        let current = self.profile_store.get_item(&uid)?;
        ensure!(
            current.itype == source.itype
                && current.file == source.file
                && current.url == source.url
                && current.option == source.option,
            "remote profile changed during download; retry refresh"
        );
        self.observe_exit().await?;
        if self.store.state().active_profile.as_deref() == Some(uid.as_str()) {
            let generation = self
                .profile_store
                .generation(&uid, runtime::parse_profile(&remote.yaml)?)?;
            let config = self.finish_generation(generation).await?;
            self.apply_with_change(
                config,
                Some(uid.clone()),
                Some(ProfileChange::Refresh(Box::new(remote))),
            )
            .await?;
        } else {
            let result = (|| {
                self.profile_store.begin_refresh(&uid, remote, None)?;
                self.profile_store.publish_refresh()
            })();
            let recovery = self.profile_store.recover_refresh(self.store.state().current.as_ref());
            if let Err(error) = result {
                return Err(error.context(format!("refresh recovery: {recovery:?}")));
            }
            recovery?;
        }
        Ok(self.profile_store.get_item(&uid)?.clone())
    }

    /// Scheduled refreshes re-check eligibility: the policy may have changed while queued.
    pub(super) async fn refresh_remote_request(
        &mut self,
        automatic: bool,
        source: PrfItem,
        remote: headless_core::config::remote::RemoteProfile,
    ) -> Result<PrfItem> {
        if automatic
            && !self
                .profile_store
                .get_item(source.uid.as_deref().unwrap_or_default())
                .is_ok_and(scheduler::eligible)
        {
            bail!("automatic subscription update disabled");
        }
        self.refresh_remote(source, remote).await
    }

    pub(super) fn delete_profile(&mut self, uid: &str) -> Result<IProfiles> {
        let result = self.profile_store.delete_profile_with_settings(
            uid,
            self.store.state().active_profile.as_deref(),
            &mut self.settings_store,
        );
        self.settings = self.settings_store.snapshot();
        if self.profile_store.get_item(uid).is_err() {
            self.dns_confirmations.remove(uid);
        }
        self.publish_profiles();
        result.map(|()| self.profile_store.snapshot())
    }

    pub(super) fn read_enhancement(&self, uid: &str, kind: ProfileEnhancement) -> Result<EnhancementContent> {
        match kind {
            ProfileEnhancement::Global(kind) => self.profile_store.read_global(kind),
            ProfileEnhancement::Sequence(kind) => self.profile_store.read_sequence(uid, kind),
            ProfileEnhancement::Merge => self.profile_store.read_merge(uid),
            ProfileEnhancement::Script => self.profile_store.read_script(uid).map(|content| EnhancementContent {
                uid: content.uid,
                yaml: content.source,
            }),
        }
    }
}
