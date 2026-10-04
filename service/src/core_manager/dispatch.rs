//! Actor mailbox handling: interrupted-journal recovery gate and per-command routing.
use super::*;

impl Actor {
    /// Finish interrupted journals before serving any command.
    async fn recover_journals(&mut self) -> Result<()> {
        self.recover_core_upgrade().await?;
        #[cfg(unix)]
        self.profile_store
            .recover_restore(&mut self.settings_store, &self.store)?;
        self.profile_store.recover_import()?;
        self.profile_store
            .recover_refresh(self.store.state().current.as_ref())?;
        self.profile_store
            .recover_enhancement(self.store.state().current.as_ref())?;
        self.settings_store.recover(self.store.state().current.as_ref())?;
        self.profile_store
            .recover_delete_with_settings(&mut self.settings_store)
    }

    pub(super) async fn handle(&mut self, request: CommandMessage) {
        if let Err(error) = self.recover_journals().await {
            let message = format!("configuration recovery failed: {error:#}");
            self.status.send_modify(|state| state.error = Some(message.clone()));
            request.fail(&message);
            return;
        }
        self.settings = self.settings_store.snapshot();
        self.dns_confirmations
            .retain(|uid, _| self.profile_store.get_item(uid).is_ok());
        self.dispatch(request).await;
    }

    /// Record a failed mutation in the published status.
    pub(super) fn record_error<T>(&self, result: &Result<T>) {
        if let Err(error) = result {
            self.status
                .send_modify(|state| state.error = Some(format!("{error:#}")));
        }
    }

    /// Publish the current profile catalog to subscribers.
    pub(super) fn publish_profiles(&self) {
        self.profile_state.send_replace(self.profile_store.snapshot());
    }

    /// Route one command. Replies are skipped when the caller already went away,
    /// and admission permits are released before replying.
    async fn dispatch(&mut self, request: CommandMessage) {
        match request {
            // Lifecycle and runtime configuration.
            CommandMessage::Control(request) => {
                let result = self.execute(request.operation).await;
                let _ = request.reply.send(result.map(|()| self.status.borrow().clone()));
            }
            CommandMessage::ReadConfig(reply) => {
                let _ = reply.send(self.store.read_current());
            }

            // Backup.
            CommandMessage::RestoreBackup {
                bytes,
                policy,
                permit,
                closing,
                mut reply,
            } => {
                if !reply.is_closed() {
                    let result = self.restore_backup(bytes, policy, closing, &mut reply).await;
                    drop(permit);
                    let _ = reply.send(result);
                }
            }
            CommandMessage::ValidateBackupRestore {
                bytes,
                permit,
                closing,
                mut reply,
            } => {
                if !reply.is_closed() {
                    let result = self.validate_backup_restore(bytes, closing, &mut reply).await;
                    drop(permit);
                    let _ = reply.send(result);
                }
            }
            CommandMessage::RetainedBackup {
                operation,
                permit,
                closing,
                mut reply,
            } => {
                if !reply.is_closed() {
                    let result = self.retained_backup(operation, permit, closing, &mut reply).await;
                    let _ = reply.send(result);
                }
            }
            CommandMessage::ExportBackup { permit, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(self.export_backup(permit).await);
                }
            }

            // Managed core upgrade.
            CommandMessage::InstalledCoreVersion(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(self.installed_version().await);
                }
            }
            CommandMessage::CheckCoreUpgrade { version, force, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(self.check_upgrade(&version, force).await);
                }
            }
            CommandMessage::UpgradePreparedCore {
                prepared,
                force,
                downloads,
                reply,
                _permit: permit,
            } => {
                if !reply.is_closed() {
                    let result = self.upgrade_prepared(prepared, force, downloads).await;
                    drop(permit);
                    let _ = reply.send(result);
                }
            }
            CommandMessage::ActivateCoreUpgrade {
                id,
                downloads,
                reply,
                _permit: permit,
            } => {
                if !reply.is_closed() {
                    let result = self.activate_core(&id, downloads).await;
                    drop(permit);
                    let _ = reply.send(result);
                }
            }
            CommandMessage::CoreInstallation(reply) => {
                let _ = reply.send(self.core_installation());
            }
            CommandMessage::StageCoreUpgrade {
                id,
                downloads,
                reply,
                _permit: permit,
            } => {
                if !reply.is_closed() {
                    let result = self.stage_core(&id, &downloads).await;
                    drop(permit);
                    let _ = reply.send(result);
                }
            }

            // Profiles and enhancements.
            CommandMessage::ImportProfile { path, name, reply } => {
                let result = self.import_profile(&path, name).await;
                self.record_error(&result);
                self.publish_profiles();
                let _ = reply.send(result);
            }
            CommandMessage::ImportProfileYaml { yaml, name, reply } => {
                let result = self.profile_store.import_local_with_defaults(&name, &yaml);
                self.record_error(&result);
                self.publish_profiles();
                let _ = reply.send(result);
            }
            CommandMessage::ImportRemote { profile, reply } => {
                if !reply.is_closed() {
                    let result = self.profile_store.import_remote_with_defaults(*profile);
                    self.publish_profiles();
                    let _ = reply.send(result);
                }
            }
            CommandMessage::RefreshRemote {
                automatic,
                source,
                profile,
                reply,
            } => {
                if !reply.is_closed() {
                    let result = self.refresh_remote_request(automatic, *source, *profile).await;
                    self.record_error(&result);
                    self.publish_profiles();
                    let _ = reply.send(result);
                }
            }
            CommandMessage::ReadProfileRaw { uid, reply } => {
                let _ = reply.send(self.profile_store.read_raw(&uid));
            }
            CommandMessage::SetProfileRaw {
                uid,
                revision,
                yaml,
                reply,
            } => {
                if !reply.is_closed() {
                    let result = self.update_profile_raw(&uid, revision, yaml).await;
                    self.record_error(&result);
                    self.publish_profiles();
                    let _ = reply.send(result);
                }
            }
            CommandMessage::EditProfile { uid, patch, reply } => {
                if !reply.is_closed() {
                    let result = self.profile_store.edit_profile(&uid, patch);
                    self.publish_profiles();
                    let _ = reply.send(result);
                }
            }
            CommandMessage::DeleteProfile { uid, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(self.delete_profile(&uid));
                }
            }
            CommandMessage::ReadEnhancement { uid, kind, reply } => {
                let _ = reply.send(self.read_enhancement(&uid, kind));
            }
            CommandMessage::SetEnhancement { uid, kind, yaml, reply } => {
                if !reply.is_closed() {
                    let result = self.set_enhancement(&uid, kind, yaml).await;
                    self.record_error(&result);
                    self.publish_profiles();
                    let _ = reply.send(result);
                }
            }

            // Settings and DNS.
            CommandMessage::ReadSettings(reply) => {
                let _ = reply.send(Ok(self.settings.clone()));
            }
            CommandMessage::SetSettings { runtime, reply } => {
                if !reply.is_closed() {
                    let result = self.update_settings(*runtime).await;
                    self.record_error(&result);
                    let _ = reply.send(result);
                }
            }
            CommandMessage::SetTunEnabled { enabled, reply } => {
                if !reply.is_closed() {
                    let mut runtime = self.settings.runtime.clone();
                    runtime.tun.get_or_insert_with(Default::default).enable = Some(enabled);
                    let result = self.update_settings(runtime).await;
                    self.record_error(&result);
                    let _ = reply.send(result);
                }
            }
            CommandMessage::ReadConnectionSettings(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(self.read_connection_settings().await);
                }
            }
            CommandMessage::ReadProfileDns { uid, reply } => {
                let _ = reply.send(self.read_profile_dns(&uid));
            }
            CommandMessage::SetProfileDns {
                uid,
                enabled,
                confirmation,
                reply,
            } => {
                let _ = reply.send(self.update_profile_dns(&uid, enabled, confirmation).await);
            }

            // Geo and resources.
            CommandMessage::ReadGeoSettings(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(self.read_geo_settings().await);
                }
            }
            CommandMessage::ReadResources(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(self.read_resources().await);
                }
            }
            CommandMessage::ValidateGeo { name, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(self.validate_geo(name).await);
                }
            }
            #[cfg(unix)]
            CommandMessage::GeoSeedInfo { name, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(self.geo_seed_info(name).await);
                }
            }
            #[cfg(unix)]
            CommandMessage::InstallGeoSeed { request, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(self.install_geo_seed(request).await);
                }
            }
            #[cfg(unix)]
            CommandMessage::GeoOnlineInfo { name, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(self.geo_online_info(&name));
                }
            }
            #[cfg(unix)]
            CommandMessage::UpdateGeoOnline { request, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(self.update_geo_online(request).await);
                }
            }
        }
    }
}
