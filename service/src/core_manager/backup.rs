//! Backup export, retained storage, validation and restore.
use super::*;

impl CoreManager {
    /// The permit is retained by the download body until completion/disconnect.
    pub async fn export_backup(&self) -> Result<crate::backup::BackupDownload> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let permit = Arc::clone(&self.backup_admission)
            .try_acquire_owned()
            .context("backup export already in progress")?;
        self.call(
            |reply| CommandMessage::ExportBackup { permit, reply },
            "backup export cancelled during shutdown",
        )
        .await
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }

    pub(crate) async fn retained_backup(
        &self,
        operation: crate::backup::RetainedOperation,
        permit: tokio::sync::OwnedSemaphorePermit,
        closing: watch::Receiver<bool>,
    ) -> Result<crate::backup::RetainedOutcome> {
        self.call(
            |reply| CommandMessage::RetainedBackup {
                operation,
                permit,
                closing,
                reply,
            },
            "local backup operation cancelled",
        )
        .await
    }

    /// Admit before buffering an upload; shares the export/download memory slot.
    pub(crate) fn admit_backup_upload(&self) -> Result<(tokio::sync::OwnedSemaphorePermit, watch::Receiver<bool>)> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let permit = Arc::clone(&self.backup_admission)
            .try_acquire_owned()
            .context("backup operation already in progress")?;
        Ok((permit, self.shutdown.subscribe()))
    }

    pub(crate) async fn validate_backup_restore(
        &self,
        bytes: axum::body::Bytes,
        permit: tokio::sync::OwnedSemaphorePermit,
        closing: watch::Receiver<bool>,
    ) -> Result<headless_core::backup::BackupRestoreValidation> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(
            bytes.len() <= headless_core::backup::MAX_ARCHIVE_BYTES,
            "backup archive exceeds 65 MiB"
        );
        self.call(
            |reply| CommandMessage::ValidateBackupRestore {
                bytes,
                permit,
                closing,
                reply,
            },
            "restore validation cancelled",
        )
        .await
    }

    pub(crate) async fn restore_backup(
        &self,
        bytes: axum::body::Bytes,
        policy: headless_core::backup::BackupRuntimePolicy,
        permit: tokio::sync::OwnedSemaphorePermit,
        closing: watch::Receiver<bool>,
    ) -> Result<headless_core::backup::BackupRestoreReceipt> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(
            bytes.len() <= headless_core::backup::MAX_ARCHIVE_BYTES,
            "backup archive exceeds 65 MiB"
        );
        self.call(
            |reply| CommandMessage::RestoreBackup {
                bytes,
                policy,
                permit,
                closing,
                reply,
            },
            "backup restore cancelled",
        )
        .await
    }
}

impl Actor {
    #[cfg(unix)]
    pub(super) async fn restore_backup(
        &mut self,
        bytes: axum::body::Bytes,
        policy: headless_core::backup::BackupRuntimePolicy,
        mut http_closing: watch::Receiver<bool>,
        reply: &mut oneshot::Sender<Result<headless_core::backup::BackupRestoreReceipt>>,
    ) -> Result<headless_core::backup::BackupRestoreReceipt> {
        // Check in the actor after recovery, so queued lifecycle commands cannot race.
        self.observe_exit().await?;
        let was_running = self.status.borrow().phase == CorePhase::Running;
        if !matches!(self.status.borrow().phase, CorePhase::Stopped | CorePhase::Running)
            || self.process.is_some() != was_running
            || self.retry_at.is_some()
        {
            return Err(crate::backup::RestoreNeedsSettled.into());
        }
        ensure!(
            !*self.shutdown.borrow() && !*http_closing.borrow() && !reply.is_closed(),
            "restore cancelled"
        );
        let (cancel, cancellation) = watch::channel(false);
        let prepared = {
            let operation =
                crate::backup::restore::publication(bytes, self.options.clone(), policy, cancellation.clone());
            tokio::pin!(operation);
            tokio::select! { biased;
                _ = closing(&mut self.shutdown) => {cancel.send_replace(true); let _ = operation.await; bail!("restore cancelled during shutdown");},
                _ = closing(&mut http_closing) => {cancel.send_replace(true); let _ = operation.await; bail!("restore cancelled during HTTP shutdown");},
                _ = reply.closed() => {cancel.send_replace(true); let _ = operation.await; bail!("restore client disconnected");},
                result = &mut operation => result?,
            }
        };
        // Durable publication stays under actor/data ownership. Cancellation is checked
        // between phases and during core I/O; after manifest commit cleanup must finish.
        let previous = self.store.state();
        let old_path = self.options.config.clone();
        let mut live_attempted = false;
        let mut restarted = false;
        let result = async {
            ensure!(
                !*self.shutdown.borrow() && !*http_closing.borrow() && !reply.is_closed(),
                "restore cancelled before commit"
            );
            let revision = self.store.stage_yaml(std::str::from_utf8(&prepared.runtime)?)?;
            let candidate = prepared.profiles()?;
            let plan = self.profile_store.prepare_restore(
                &candidate,
                &self.settings_store,
                prepared.settings.clone(),
                &self.store,
                revision.clone(),
            )?;
            ensure!(
                !*self.shutdown.borrow() && !*http_closing.borrow() && !reply.is_closed(),
                "restore cancelled before commit"
            );
            self.store
                .begin_profile(revision.clone(), plan.active_profile().map(str::to_owned))?;
            self.profile_store
                .begin_restore(plan, &self.settings_store, &self.store)?;
            ensure!(
                !*self.shutdown.borrow() && !*http_closing.borrow() && !reply.is_closed(),
                "restore cancelled before commit"
            );
            if was_running {
                self.cancel_restoration();
                live_attempted = true;
                restarted = self
                    .restore_live_runtime(
                        self.store.path(&revision)?,
                        &cancel,
                        cancellation,
                        http_closing.clone(),
                        reply,
                    )
                    .await?;
            }
            // The original manager shutdown receiver has been restored after core I/O.
            ensure!(
                !*self.shutdown.borrow() && !*http_closing.borrow() && !reply.is_closed(),
                "restore cancelled before commit"
            );
            self.profile_store
                .publish_restore(&mut self.settings_store, &self.store)?;
            ensure!(
                !*self.shutdown.borrow() && !*http_closing.borrow() && !reply.is_closed(),
                "restore cancelled before commit"
            );
            let commit = self.store.commit();
            if self.store.state().current.as_ref() != Some(&revision) {
                commit?;
                bail!("restore did not commit");
            }
            Ok::<_, anyhow::Error>((revision, commit.is_err()))
        }
        .await;
        match result {
            Ok((revision, mut cleanup_pending)) => {
                self.options.config = self.store.path(&revision)?;
                self.retry_at = None;
                self.cancel_restoration();
                self.dns_confirmations.clear();
                cleanup_pending |= self
                    .profile_store
                    .recover_restore(&mut self.settings_store, &self.store)
                    .is_err();
                cleanup_pending |= prepared.cleanup().is_err();
                self.settings = self.settings_store.snapshot();
                self.profile_state.send_replace(self.profile_store.snapshot());
                self.status.send_modify(|state| {
                    state.config_revision = Some(revision.file.clone());
                    state.active_profile = self.store.state().active_profile;
                    state.error = cleanup_pending.then(|| {
                        "backup restore committed; cleanup or durability acknowledgement requires recovery".into()
                    });
                    state.selection_pending.clear();
                    state.selection_error = None;
                    state.recovery_attempt = 0;
                });
                if was_running {
                    self.begin_restoration(false).await;
                }
                if cleanup_pending {
                    self.status.send_modify(|state| {
                        state.error =
                            Some("backup restore committed; recovery or durability acknowledgement is pending".into())
                    });
                }
                Ok(headless_core::backup::BackupRestoreReceipt {
                    committed: true,
                    core_running: self.status.borrow().phase == CorePhase::Running,
                    core_restarted: restarted,
                    archive: prepared.report.archive.clone(),
                    runtime_policy: policy,
                    runtime_revision: revision.file,
                    runtime_bytes: prepared.runtime.len() as u64,
                    runtime_sha256: crate::backup::hash(&prepared.runtime),
                    dns_override_requires_confirmation: prepared.report.dns_override_requires_confirmation,
                    cleanup_pending,
                })
            }
            Err(error) => {
                self.options.config = old_path;
                let rollback = self.store.restore(previous).and_then(|()| {
                    self.profile_store
                        .recover_restore(&mut self.settings_store, &self.store)
                });
                let core_recovery = if live_attempted {
                    self.publish(CorePhase::Stopping, None);
                    match self.stop_process().await {
                        Ok(()) if rollback.is_ok() && !*self.shutdown.borrow() => self.start_inner().await,
                        result => result,
                    }
                } else {
                    Ok(())
                };
                if live_attempted && (core_recovery.is_err() || rollback.is_err()) {
                    let _ = self.stop_process().await;
                    self.publish(
                        CorePhase::Failed,
                        Some("backup restore failed; core recovery is pending".into()),
                    );
                } else if live_attempted && *self.shutdown.borrow() {
                    self.publish(CorePhase::Stopped, None);
                }
                let cleanup = prepared.cleanup();
                self.settings = self.settings_store.snapshot();
                self.profile_state.send_replace(self.profile_store.snapshot());
                if live_attempted && rollback.is_ok() && core_recovery.is_ok() && !*self.shutdown.borrow() {
                    self.begin_restoration(false).await;
                }
                if rollback.is_err() || cleanup.is_err() || core_recovery.is_err() {
                    self.status
                        .send_modify(|state| state.error = Some("backup restore failed; recovery is pending".into()));
                }
                // Uploaded probe diagnostics are excluded from HTTP errors and live logs.
                Err(error.context(format!(
                    "restore recovery: {}; core recovery: {}; candidate cleanup: {}",
                    rollback.is_ok(),
                    core_recovery.is_ok(),
                    cleanup.is_ok()
                )))
            }
        }
    }

    /// Bridge private HTTP/disconnect cancellation into the existing core I/O checks.
    /// Always join stop/start/reload; never drop a future owning an unreaped child.
    #[cfg(unix)]
    pub(super) async fn restore_live_runtime(
        &mut self,
        path: PathBuf,
        cancel: &watch::Sender<bool>,
        cancellation: watch::Receiver<bool>,
        mut http_closing: watch::Receiver<bool>,
        reply: &mut oneshot::Sender<Result<headless_core::backup::BackupRestoreReceipt>>,
    ) -> Result<bool> {
        let original = std::mem::replace(&mut self.shutdown, cancellation);
        let mut manager_shutdown = original.clone();
        let result = {
            let operation = async {
                let reload = self.reload(&path).await;
                ensure!(!*self.shutdown.borrow(), "live restore cancelled");
                if reload.is_ok() {
                    return Ok(false);
                }
                self.publish(CorePhase::Stopping, None);
                self.stop_process().await?;
                ensure!(!*self.shutdown.borrow(), "live restore cancelled before restart");
                self.options.config = path;
                self.publish(CorePhase::Starting, None);
                self.start_inner().await?;
                ensure!(!*self.shutdown.borrow(), "live restore cancelled after restart");
                Ok(true)
            };
            tokio::pin!(operation);
            tokio::select! { biased;
                _ = closing(&mut manager_shutdown) => {cancel.send_replace(true); let _ = operation.await; Err(anyhow::anyhow!("live restore cancelled during shutdown"))},
                _ = closing(&mut http_closing) => {cancel.send_replace(true); let _ = operation.await; Err(anyhow::anyhow!("live restore cancelled during HTTP shutdown"))},
                _ = reply.closed() => {cancel.send_replace(true); let _ = operation.await; Err(anyhow::anyhow!("live restore client disconnected"))},
                result = &mut operation => result,
            }
        };
        self.shutdown = original;
        result
    }

    #[cfg(not(unix))]
    pub(super) async fn restore_backup(
        &mut self,
        _bytes: axum::body::Bytes,
        _policy: headless_core::backup::BackupRuntimePolicy,
        _closing: watch::Receiver<bool>,
        _reply: &mut oneshot::Sender<Result<headless_core::backup::BackupRestoreReceipt>>,
    ) -> Result<headless_core::backup::BackupRestoreReceipt> {
        bail!("backup restoration is not yet supported on this platform")
    }

    #[cfg(unix)]
    pub(super) async fn validate_backup_restore(
        &mut self,
        bytes: axum::body::Bytes,
        mut http_closing: watch::Receiver<bool>,
        reply: &mut oneshot::Sender<Result<headless_core::backup::BackupRestoreValidation>>,
    ) -> Result<headless_core::backup::BackupRestoreValidation> {
        ensure!(
            !*self.shutdown.borrow() && !*http_closing.borrow(),
            "restore validation cancelled"
        );
        let (cancel, cancellation) = watch::channel(false);
        let operation = crate::backup::restore::validate(bytes, self.options.clone(), cancellation);
        tokio::pin!(operation);
        tokio::select! { biased;
            _ = closing(&mut self.shutdown) => { cancel.send_replace(true); let _ = operation.await; bail!("restore validation cancelled during shutdown"); },
            _ = closing(&mut http_closing) => { cancel.send_replace(true); let _ = operation.await; bail!("restore validation cancelled during HTTP shutdown"); },
            _ = reply.closed() => { cancel.send_replace(true); let _ = operation.await; bail!("restore validation client disconnected"); },
            result = &mut operation => result,
        }
    }

    #[cfg(not(unix))]
    pub(super) async fn validate_backup_restore(
        &mut self,
        _bytes: axum::body::Bytes,
        _closing: watch::Receiver<bool>,
        _reply: &mut oneshot::Sender<Result<headless_core::backup::BackupRestoreValidation>>,
    ) -> Result<headless_core::backup::BackupRestoreValidation> {
        bail!("restore validation is not yet supported on this platform")
    }

    #[cfg(target_os = "linux")]
    pub(super) async fn retained_backup(
        &mut self,
        operation: crate::backup::RetainedOperation,
        permit: tokio::sync::OwnedSemaphorePermit,
        mut http_closing: watch::Receiver<bool>,
        reply: &mut oneshot::Sender<Result<crate::backup::RetainedOutcome>>,
    ) -> Result<crate::backup::RetainedOutcome> {
        use crate::backup::{RetainedOperation, RetainedOutcome, storage::Output};
        ensure!(
            !*self.shutdown.borrow() && !*http_closing.borrow() && !reply.is_closed(),
            "local backup cancelled"
        );
        let (snapshot, permit) = if matches!(operation, RetainedOperation::Create) {
            let download = self.export_backup(permit).await?;
            (Some((download.metadata, download.bytes)), download.permit)
        } else {
            (None, permit)
        };
        ensure!(
            !*self.shutdown.borrow() && !*http_closing.borrow() && !reply.is_closed(),
            "local backup cancelled"
        );
        let data = self.options.data_dir.clone();
        let (cancel, cancellation) = watch::channel(false);
        let mut worker =
            tokio::task::spawn_blocking(move || crate::backup::storage::run(&data, operation, snapshot, cancellation));
        let result = tokio::select! {biased;
            _=closing(&mut self.shutdown)=>{cancel.send_replace(true);worker.await},
            _=closing(&mut http_closing)=>{cancel.send_replace(true);worker.await},
            _=reply.closed()=>{cancel.send_replace(true);worker.await},
            result=&mut worker=>result,
        }
        .context("local backup worker failed")??;
        // A logical create/delete commit survives cancellation and fsync acknowledgement errors.
        Ok(match result {
            Output::Created(receipt) => RetainedOutcome::Created(receipt),
            Output::Listed(list) => RetainedOutcome::Listed(list),
            Output::Deleted(receipt) => RetainedOutcome::Deleted(receipt),
            Output::Downloaded(metadata, bytes) => RetainedOutcome::Downloaded(crate::backup::BackupDownload {
                metadata,
                bytes,
                permit,
            }),
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) async fn retained_backup(
        &mut self,
        _operation: crate::backup::RetainedOperation,
        _permit: tokio::sync::OwnedSemaphorePermit,
        _closing: watch::Receiver<bool>,
        _reply: &mut oneshot::Sender<Result<crate::backup::RetainedOutcome>>,
    ) -> Result<crate::backup::RetainedOutcome> {
        bail!("retained backups are not supported on this platform")
    }

    #[cfg(unix)]
    pub(super) async fn export_backup(
        &mut self,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<crate::backup::BackupDownload> {
        let state = self.store.state();
        ensure!(state.pending.is_none(), "backup requires a committed runtime snapshot");
        let snapshot = crate::backup::export::Snapshot {
            data_dir: self.options.data_dir.clone(),
            runtime_path: self.options.config.clone(),
            profiles: self.profile_store.snapshot(),
            settings: self.settings.clone(),
            runtime_revision: state.current.map(|r| r.file),
            active_profile: state.active_profile,
        };
        let cancellation = self.shutdown.clone();
        let mut worker = tokio::task::spawn_blocking(move || crate::backup::export::build(snapshot, cancellation));
        let (metadata, bytes) = tokio::select! {biased;
            _ = closing(&mut self.shutdown) => {
                // A blocking reader must finish before releasing directory ownership.
                let _ = worker.await;
                bail!("backup export cancelled during shutdown");
            },
            result = &mut worker => result.context("backup worker failed")??,
        };
        ensure!(!*self.shutdown.borrow(), "backup export cancelled during shutdown");
        Ok(crate::backup::BackupDownload {
            metadata,
            bytes,
            permit,
        })
    }

    #[cfg(not(unix))]
    pub(super) async fn export_backup(
        &mut self,
        _permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<crate::backup::BackupDownload> {
        bail!("backup export is not yet supported on this platform")
    }
}
