//! Core process supervision: start, stop, reload, exit observation and recovery.
use super::*;

pub(super) struct ManagedProcess {
    pub(super) child: Child,
    pub(super) readers: Vec<JoinHandle<()>>,
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        for reader in &self.readers {
            reader.abort();
        }
    }
}

impl CoreManager {
    pub async fn runtime_config(&self) -> Result<Mapping> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            CommandMessage::ReadConfig,
            "configuration read cancelled during shutdown",
        )
        .await
    }

    pub(super) async fn request(&self, operation: Operation) -> Result<CoreStatus> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        self.call(
            |reply| CommandMessage::Control(Request { operation, reply }),
            "core operation cancelled during shutdown",
        )
        .await
    }

    pub async fn start(&self) -> Result<CoreStatus> {
        self.request(Operation::Start).await
    }

    pub async fn stop(&self) -> Result<CoreStatus> {
        self.request(Operation::Stop).await
    }

    pub async fn restart(&self) -> Result<CoreStatus> {
        self.request(Operation::Restart).await
    }

    pub async fn reload_config(&self, path: PathBuf) -> Result<CoreStatus> {
        self.request(Operation::Reload(path)).await
    }

    pub async fn apply_config(&self, config: Mapping) -> Result<CoreStatus> {
        self.request(Operation::Apply(Box::new(config))).await
    }

    /// Replace edited runtime YAML while preserving the active profile association.
    pub async fn edit_config(&self, config: Mapping) -> Result<CoreStatus> {
        self.request(Operation::Edit(Box::new(config))).await
    }

    pub async fn import_config(&self, path: PathBuf) -> Result<CoreStatus> {
        self.request(Operation::Import(path)).await
    }

    /// Merge settings into the committed config using the extracted upstream helper.
    pub async fn apply_overlay(&self, overlay: Mapping) -> Result<CoreStatus> {
        self.request(Operation::Merge(Box::new(overlay))).await
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown.send_replace(true);
        let mut scheduler = self.scheduler_completion.clone();
        while !*scheduler.borrow_and_update() {
            scheduler
                .changed()
                .await
                .context("subscription scheduler ended without completion")?;
        }
        let mut done = self.completion.clone();
        loop {
            if let Some(result) = done.borrow().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            done.changed()
                .await
                .context("core supervisor ended without a shutdown result")?;
        }
    }
}

impl Actor {
    pub(super) async fn execute(&mut self, operation: Operation) -> Result<()> {
        self.observe_exit().await?;
        match operation {
            Operation::Start => {
                self.retry_at = None;
                if self.process.is_some() && self.status.borrow().phase == CorePhase::Running {
                    return Ok(());
                }
                self.status.send_modify(|state| state.recovery_attempt = 0);
                self.start_core().await
            }
            Operation::Stop | Operation::Restart => {
                self.retry_at = None;
                let restart = matches!(operation, Operation::Restart);
                self.status.send_modify(|state| state.recovery_attempt = 0);
                self.publish(CorePhase::Stopping, None);
                if let Err(error) = self.stop_process().await {
                    self.publish(CorePhase::Failed, Some(format!("{error:#}")));
                    return Err(error);
                }
                self.publish(CorePhase::Stopped, None);
                if restart { self.start_core().await } else { Ok(()) }
            }
            Operation::Reload(path) => {
                ensure!(self.status.borrow().phase == CorePhase::Running, "core is not running");
                self.apply_file(&path).await
            }
            Operation::Import(path) => self.apply_file(&path).await,
            // Apply replaces the runtime without a profile; Edit keeps the active profile.
            Operation::Apply(config) => {
                let result = self.apply(*config, None).await;
                self.record_error(&result);
                result
            }
            Operation::Edit(config) => {
                let active_profile = self.store.state().active_profile;
                let result = self.apply(*config, active_profile).await;
                self.record_error(&result);
                result
            }
            Operation::Merge(overlay) => {
                let result = async {
                    let config = runtime::generate(self.store.read_current()?, &overlay)?;
                    let active_profile = self.store.state().active_profile;
                    self.apply(config, active_profile).await
                }
                .await;
                self.record_error(&result);
                result
            }
            Operation::SelectProfile(uid) => {
                let result = async {
                    let generation = self.profile_store.read_generation(&uid)?;
                    let config = self.finish_generation(generation).await?;
                    self.apply(config, Some(uid)).await
                }
                .await;
                self.record_error(&result);
                result
            }
            Operation::SelectNode { group, node } => self.change_node(&group, Some(&node)).await,
            Operation::UnfixNode(group) => self.change_node(&group, None).await,
        }
    }

    async fn apply_file(&mut self, path: &Path) -> Result<()> {
        let result = async {
            let config = read_config(path).await?;
            self.apply(config, None).await
        }
        .await;
        self.record_error(&result);
        result
    }

    pub(super) async fn reload(&mut self, path: &Path) -> Result<()> {
        let path = tokio::fs::canonicalize(path).await?;
        check_config(&path).await?;
        crate::validation::resource_paths(&self.options.data_dir, &path, &self.options.binary).await?;
        self.tun_preflight(&read_config(&path).await?)?;
        tokio::select! {
            biased;
            _ = closing(&mut self.shutdown) => bail!("reload cancelled during shutdown"),
            result = self.client.reload_config(true, path.to_str().context("config path must be UTF-8")?) => result?,
        }
        self.verify_proxy_ports(&path).await?;
        self.options.config = path;
        self.status.send_modify(|state| state.error = None);
        Ok(())
    }

    /// Host TUN checks, plus this user's TUN authorization in multi-user mode.
    pub(super) fn tun_preflight(&self, config: &Mapping) -> Result<()> {
        crate::native_tun::preflight(config)?;
        let enabled = config
            .get("tun")
            .and_then(|tun| tun.get("enable"))
            .and_then(serde_yaml_ng::Value::as_bool)
            == Some(true);
        ensure!(
            !enabled || self.options.tun_capable != Some(false),
            "TUN is not available to this user: ask the administrator to add this user to the \
             TUN group (mihomo-tun)"
        );
        // The first user to enable the host's one system-wide TUN keeps it until off.
        #[cfg(target_os = "linux")]
        if enabled && let Some(lock) = &self.options.tun_lock {
            lock.acquire()?;
        }
        Ok(())
    }

    pub(super) async fn verify_proxy_ports(&mut self, path: &Path) -> Result<()> {
        // The API can answer before proxy listeners or TUN have finished
        // starting. Poll both, while still rejecting a persistent bind failure.
        const LISTENER_SETTLE: Duration = Duration::from_secs(4);
        let config = read_config(path).await?;
        let tun_expected = config
            .get("tun")
            .and_then(|tun| tun.get("enable"))
            .and_then(serde_yaml_ng::Value::as_bool)
            == Some(true);
        let deadline = Instant::now() + LISTENER_SETTLE;
        loop {
            let core = tokio::select! {
                biased;
                _ = closing(&mut self.shutdown) => bail!("listener verification cancelled during shutdown"),
                result = timeout(self.options.policy.probe_timeout, self.client.get_base_config()) => {
                    result.context("listener verification timed out")??
                }
            };
            let verified = crate::proxy_access::verify_ports(&config, &core)
                .and_then(|()| crate::native_tun::verify(&config, &core));
            match verified {
                Err(_) if Instant::now() < deadline => {}
                Ok(()) if tun_expected => {
                    self.tun_live.store(true, std::sync::atomic::Ordering::Relaxed);
                    return self.await_system_dns(&core.tun.device).await;
                }
                Ok(()) => {
                    self.tun_live.store(false, std::sync::atomic::Ordering::Relaxed);
                    self.release_idle_tun();
                    return Ok(());
                }
                result => return result,
            }
            if let Some(process) = self.process.as_mut() {
                ensure!(
                    process.child.try_wait()?.is_none(),
                    "Mihomo exited while listeners were starting"
                );
            }
            tokio::select! {
                biased;
                _ = closing(&mut self.shutdown) => bail!("listener verification cancelled during shutdown"),
                _ = sleep(Duration::from_millis(250)) => {}
            }
        }
    }

    /// A system-wide TUN is complete only once Mihomo's background `resolvectl`
    /// calls have pointed systemd-resolved at it; report the toggle after that.
    /// Hosts without systemd-resolved have nothing to wait for. A resolver that
    /// never accepts the link (no polkit rule) keeps the TUN and logs why.
    async fn await_system_dns(&mut self, device: &str) -> Result<()> {
        #[cfg(target_os = "linux")]
        if self.options.isolation.map(|isolation| isolation.tun_scope())
            == Some(headless_core::enhance::isolation::TunScope::System)
        {
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                match crate::native_tun::resolved_link_dns(device).await {
                    None | Some(true) => return Ok(()),
                    Some(false) if Instant::now() >= deadline => {
                        self.logs.append(
                            "manager",
                            format!(
                                "systemd-resolved did not accept DNS for {device}; system lookups bypass \
                                 the TUN until the installer's polkit rule is present"
                            ),
                        );
                        return Ok(());
                    }
                    Some(false) => {}
                }
                tokio::select! {
                    biased;
                    _ = closing(&mut self.shutdown) => bail!("TUN verification cancelled during shutdown"),
                    _ = sleep(Duration::from_millis(100)) => {}
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = device;
        Ok(())
    }

    /// Starting with TUN while another user holds the host's system TUN (e.g. at
    /// boot): turn this user's TUN setting off and start without it, rather than
    /// fail. The user may enable it again once the holder turns theirs off.
    fn yield_system_tun(&mut self) -> Result<bool> {
        #[cfg(target_os = "linux")]
        if let Some(lock) = &self.options.tun_lock
            && self.options.tun_capable == Some(true)
            && self.store.read_current().is_ok_and(|config| {
                config
                    .get("tun")
                    .and_then(|tun| tun.get("enable"))
                    .and_then(serde_yaml_ng::Value::as_bool)
                    == Some(true)
            })
            && let Err(error) = lock.acquire()
        {
            let mut settings = self.settings.clone();
            settings.runtime.tun.get_or_insert_with(Default::default).enable = Some(false);
            self.settings_store.replace(settings)?;
            self.settings = self.settings_store.snapshot();
            self.logs.append("manager", format!("TUN turned off: {error:#}"));
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) async fn start_core(&mut self) -> Result<()> {
        if self.options.managed_core() && self.process.is_none() {
            crate::core_upgrade::recover(self.options.binary.parent().context("managed core directory missing")?)?;
        }
        self.cancel_restoration();
        self.publish(CorePhase::Starting, None);
        let previous = self.store.state();
        let bootstrap = previous.current.is_none() && previous.pending.is_none();
        let source = self.options.config.clone();
        let yielded = self.yield_system_tun()?;
        // Multi-user mode: a runtime committed before isolation (or for another
        // slot) is re-staged so the core never starts with conflicting listeners.
        let restage = yielded
            || match &self.options.isolation {
                // An unreadable runtime fails below with the ordinary start error.
                Some(isolation) if previous.current.is_some() && previous.pending.is_none() => self
                    .store
                    .read_current()
                    .is_ok_and(|current| isolation.apply(current.clone(), &self.settings.runtime).0 != current),
                _ => false,
            };
        let result = async {
            if bootstrap {
                let config = read_config(&source).await?;
                self.options.config = self.stage(config.into(), None).await?.0;
            } else if restage {
                let config = self.store.read_current()?;
                self.options.config = self.stage(config.into(), previous.active_profile.clone()).await?.0;
            }
            self.start_inner().await?;
            if bootstrap || restage {
                self.store.commit()?;
                self.status.send_modify(|state| {
                    state.config_revision = self.store.state().current.map(|revision| revision.file);
                });
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            let mut error = error;
            if bootstrap || restage {
                self.options.config = source;
                if let Err(rollback) = self.store.restore(previous) {
                    error = error.context(format!("initial configuration rollback failed: {rollback:#}"));
                }
            }
            let cleanup = self.stop_process().await;
            let message = match cleanup {
                Ok(()) => format!("{error:#}"),
                Err(cleanup) => format!("{error:#}; cleanup failed: {cleanup:#}"),
            };
            self.publish(CorePhase::Failed, Some(message.clone()));
            return Err(anyhow::Error::msg(message));
        }
        if self.store.state().pending.is_none() {
            self.begin_restoration(false).await;
        }
        Ok(())
    }

    pub(super) async fn start_inner(&mut self) -> Result<()> {
        ensure!(
            self.process.is_none(),
            "previous core must be stopped before another is spawned"
        );
        check_config(&self.options.config).await?;
        crate::validation::resource_paths(&self.options.data_dir, &self.options.config, &self.options.binary).await?;
        self.tun_preflight(&read_config(&self.options.config).await?)?;
        if self.store.state().pending.is_none() {
            crate::validation::validate(
                &self.options.binary,
                &self.options.data_dir,
                &self.options.config,
                &mut self.shutdown,
                self.options.policy.validation_timeout,
            )
            .await?;
        }
        #[cfg(unix)]
        if tokio::fs::try_exists(&self.socket).await? {
            use std::os::unix::fs::FileTypeExt as _;
            ensure!(
                tokio::fs::symlink_metadata(&self.socket).await?.file_type().is_socket(),
                "controller path is not a socket"
            );
            match tokio::net::UnixStream::connect(&self.socket).await {
                Ok(_) => bail!("controller socket is already in use"),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    tokio::fs::remove_file(&self.socket).await?;
                }
                Err(error) => return Err(error).context("cannot inspect existing controller socket"),
            }
        }
        let mut command = match &self.options.tun_exec {
            Some(launcher) => {
                let mut command = Command::new(launcher);
                command.arg(std::process::id().to_string()).arg(&self.options.binary);
                command
            }
            None => Command::new(&self.options.binary),
        };
        crate::shutdown::bind_child_lifetime(&mut command);
        let mut child = command
            .arg("-d")
            .arg(&self.options.data_dir)
            .arg("-f")
            .arg(&self.options.config)
            .arg(if cfg!(windows) {
                "-ext-ctl-pipe"
            } else {
                "-ext-ctl-unix"
            })
            .arg(&self.socket)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn Mihomo")?;
        let stdout = child.stdout.take().context("Mihomo stdout unavailable")?;
        let stderr = child.stderr.take().context("Mihomo stderr unavailable")?;
        let readers = vec![
            drain(stdout, self.logs.clone(), "stdout"),
            drain(stderr, self.logs.clone(), "stderr"),
        ];
        self.process = Some(ManagedProcess { child, readers });
        self.publish(CorePhase::Starting, None);
        let mut last_error = String::from("no readiness response");
        for _ in 0..self.options.policy.readiness_attempts {
            if let Some(exit) = self
                .process
                .as_mut()
                .context("core process missing")?
                .child
                .try_wait()?
            {
                self.status.send_modify(|state| state.exit_code = exit.code());
                bail!("Mihomo exited before readiness: {exit}");
            }
            let probe = tokio::select! {
                biased;
                _ = closing(&mut self.shutdown) => bail!("startup cancelled during shutdown"),
                probe = timeout(self.options.policy.probe_timeout, self.client.get_version()) => probe,
            };
            match probe {
                Ok(Ok(version)) => {
                    self.verify_proxy_ports(&self.options.config.clone()).await?;
                    self.status.send_modify(|state| {
                        state.phase = CorePhase::Running;
                        state.pid = self.process.as_ref().and_then(|process| process.child.id());
                        state.error = None;
                        state.version = Some(version.version);
                        state.generation += 1;
                    });
                    return Ok(());
                }
                Ok(Err(error)) => last_error = error.to_string(),
                Err(error) => last_error = error.to_string(),
            }
            tokio::select! {
                biased;
                _ = closing(&mut self.shutdown) => bail!("startup cancelled during shutdown"),
                _ = sleep(self.options.policy.probe_interval) => {}
            }
        }
        bail!("Mihomo API did not become ready: {last_error}")
    }

    pub(super) async fn stop_process(&mut self) -> Result<()> {
        self.cancel_restoration();
        self.client.clear_all_ws_connections()?;
        let Some(process) = &mut self.process else {
            return Ok(());
        };
        let exit = match process.child.try_wait()? {
            Some(exit) => exit,
            None => {
                #[cfg(unix)]
                if let Some(pid) = process.child.id() {
                    // The actor owns this unreaped child, so the PID cannot be reused here.
                    let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                    if sent != 0 {
                        self.logs.append(
                            "manager",
                            format!("SIGTERM failed: {}", std::io::Error::last_os_error()),
                        );
                    }
                }
                #[cfg(not(unix))]
                process.child.start_kill()?;
                match timeout(self.options.policy.stop_timeout, process.child.wait()).await {
                    Ok(result) => result?,
                    Err(_) => {
                        self.logs
                            .append("manager", "graceful stop timed out; forcing termination".to_owned());
                        process.child.start_kill()?;
                        timeout(self.options.policy.kill_timeout, process.child.wait())
                            .await
                            .context("timed out reaping Mihomo")??
                    }
                }
            }
        };
        self.status.send_modify(|state| state.exit_code = exit.code());
        for reader in &mut process.readers {
            if timeout(Duration::from_secs(1), &mut *reader).await.is_err() {
                reader.abort();
                let _ = reader.await;
            }
        }
        self.process = None;
        Ok(())
    }

    pub(super) async fn observe_exit(&mut self) -> Result<()> {
        if self.status.borrow().phase != CorePhase::Running {
            return Ok(());
        }
        let exit: Option<ExitStatus> = match &mut self.process {
            Some(process) => process.child.try_wait()?,
            None => None,
        };
        if let Some(exit) = exit {
            self.stop_process().await?;
            self.publish(CorePhase::Failed, Some(format!("Mihomo exited unexpectedly: {exit}")));
            self.schedule_recovery();
        }
        Ok(())
    }

    pub(super) fn schedule_recovery(&mut self) {
        if *self.shutdown.borrow() || self.process.is_some() {
            return;
        }
        let mut state = self.status.borrow().clone();
        if state.recovery_attempt >= self.options.policy.recovery_limit {
            return;
        }
        state.recovery_attempt += 1;
        state.phase = CorePhase::Recovering;
        let multiplier = 1_u32 << (state.recovery_attempt - 1);
        self.retry_at = Some(Instant::now() + self.options.policy.recovery_backoff * multiplier);
        self.status.send_replace(state);
    }
}
