//! Node selection and saved-selection restoration after core (re)starts.
use super::*;

pub(super) struct Restoration {
    pub(super) uid: String,
    pub(super) selected: Vec<PrfSelected>,
    pub(super) previous: Option<Proxies>,
    pub(super) completed: HashMap<smartstring::alias::String, smartstring::alias::String>,
    pub(super) next_at: Instant,
    pub(super) deadline: Instant,
    pub(super) prune: bool,
}

impl CoreManager {
    pub async fn select_node(&self, group: String, node: String) -> Result<CoreStatus> {
        self.request(Operation::SelectNode { group, node }).await
    }

    pub async fn unfix_node(&self, group: String) -> Result<CoreStatus> {
        self.request(Operation::UnfixNode(group)).await
    }
}

impl Actor {
    pub(super) fn cancel_restoration(&mut self) {
        self.restoration = None;
        self.status.send_modify(|state| {
            state.selection_pending.clear();
            state.selection_error = None;
        });
    }

    pub(super) async fn selection_call<T>(
        &mut self,
        deadline: Instant,
        task: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let deadline = deadline.min(Instant::now() + self.options.policy.selection_timeout);
        tokio::select! {
            biased;
            _ = closing(&mut self.shutdown) => bail!("node operation cancelled during shutdown"),
            result = tokio::time::timeout_at(deadline, task) => result.context("node operation timeout")?,
        }
    }

    pub(super) async fn proxy_snapshot(&mut self, deadline: Instant) -> Result<Proxies> {
        let client = self.client.clone();
        self.selection_call(deadline, async move { Ok(client.get_proxies().await?) })
            .await
    }

    pub(super) async fn set_runtime_node(&mut self, group: &str, node: Option<&str>, deadline: Instant) -> Result<()> {
        let client = self.client.clone();
        self.selection_call(deadline, async move {
            match node {
                Some(node) => client.select_node_for_group(group, node).await?,
                None => client.unfixed_proxy(group).await?,
            }
            Ok(())
        })
        .await
    }

    pub(super) async fn change_node(&mut self, group: &str, node: Option<&str>) -> Result<()> {
        let result = self.change_node_inner(group, node).await;
        if let Err(error) = &result {
            self.status.send_modify(|state| {
                state.error = Some(format!("{error:#}"));
                state.selection_error = Some(format!("{error:#}"));
            });
        }
        result
    }

    pub(super) async fn change_node_inner(&mut self, group: &str, node: Option<&str>) -> Result<()> {
        ensure!(self.status.borrow().phase == CorePhase::Running, "core is not running");
        let uid = self
            .store
            .state()
            .active_profile
            .context("select an active profile before choosing a node")?;
        let recorded = self.profile_store.selections(&uid)?;
        let snapshot = self
            .proxy_snapshot(Instant::now() + self.options.policy.selection_timeout)
            .await?;
        let proxy = snapshot.proxies.get(group).context("proxy group does not exist")?;
        let automatic = matches!(
            proxy.proxy_type,
            ProxyType::URLTest | ProxyType::Fallback | ProxyType::LoadBalance
        );
        ensure!(
            automatic || proxy.proxy_type == ProxyType::Selector,
            "proxy is not a selectable group"
        );
        if let Some(node) = node {
            ensure!(
                proxy
                    .all
                    .as_ref()
                    .is_some_and(|all| all.iter().any(|member| member == node)),
                "node is not a member of this group"
            );
        } else {
            ensure!(automatic, "only automatic groups support removing a fixed node");
        }
        let previous = if automatic {
            proxy.fixed.clone()
        } else {
            Some(proxy.now.clone().context("previous group selection is unavailable")?)
        };
        self.cancel_restoration();
        let result = async {
            self.set_runtime_node(group, node, Instant::now() + self.options.policy.selection_timeout)
                .await?;
            let snapshot = self
                .proxy_snapshot(Instant::now() + self.options.policy.selection_timeout)
                .await?;
            let actual = snapshot
                .proxies
                .get(group)
                .context("group disappeared after node operation")?;
            match node {
                Some(node) => ensure!(
                    actual.now.as_deref() == Some(node) || (automatic && actual.fixed.as_deref() == Some(node)),
                    "core did not confirm requested node"
                ),
                None => ensure!(
                    actual.fixed.as_deref().is_none_or(str::is_empty),
                    "core did not remove the fixed node"
                ),
            }
            ensure!(!*self.shutdown.borrow(), "node persistence cancelled during shutdown");
            match node {
                Some(node) => self.profile_store.record_selection(&uid, group, node)?,
                None => self.profile_store.forget_selection(&uid, group)?,
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            let disk = self.profile_store.save_selections(&uid, recorded);
            let runtime = if *self.shutdown.borrow() {
                Ok(())
            } else {
                self.set_runtime_node(
                    group,
                    previous.as_deref(),
                    Instant::now() + self.options.policy.selection_timeout,
                )
                .await
            };
            self.profile_state.send_replace(self.profile_store.snapshot());
            let mut message = format!("node operation failed: {error:#}");
            if let Err(error) = disk {
                message.push_str(&format!("; selection record rollback failed: {error:#}"));
            }
            if let Err(error) = runtime {
                message.push_str(&format!("; runtime node rollback failed: {error:#}"));
            }
            bail!(message);
        }
        self.profile_state.send_replace(self.profile_store.snapshot());
        self.status.send_modify(|state| {
            state.error = None;
            state.selection_error = None;
        });
        Ok(())
    }

    pub(super) async fn begin_restoration(&mut self, prune: bool) {
        self.cancel_restoration();
        if *self.shutdown.borrow() || self.status.borrow().phase != CorePhase::Running {
            return;
        }
        let Some(uid) = self.store.state().active_profile else {
            return;
        };
        let selected = match self.profile_store.selections(&uid) {
            Ok(selected) => selected,
            Err(error) => {
                self.status.send_modify(|state| {
                    state.selection_error = Some(format!("cannot read saved selections: {error:#}"))
                });
                return;
            }
        };
        if selected.is_empty() {
            return;
        }
        self.status.send_modify(|state| {
            state.selection_pending = selected
                .iter()
                .filter_map(|record| record.name.as_ref().map(ToString::to_string))
                .collect();
        });
        let now = Instant::now();
        self.restoration = Some(Restoration {
            uid,
            selected,
            previous: None,
            completed: HashMap::new(),
            next_at: now,
            deadline: now + self.options.policy.selection_settle,
            prune,
        });
        self.restore_step(now + self.options.policy.selection_first_pass).await;
    }

    pub(super) async fn restore_step(&mut self, budget: Instant) {
        let Some(mut restore) = self.restoration.take() else {
            return;
        };
        if self.status.borrow().phase != CorePhase::Running
            || self.store.state().active_profile.as_deref() != Some(&restore.uid)
            || *self.shutdown.borrow()
        {
            return;
        }
        let deadline = budget.min(restore.deadline);
        let result = async {
            let mut snapshot = self.proxy_snapshot(deadline).await?;
            // Retry a previously accepted selection if the core subsequently moved away.
            restore.completed.retain(|group, node| {
                snapshot
                    .proxies
                    .get(group.as_str())
                    .is_some_and(|proxy| proxy.now.as_deref() == Some(node.as_str()))
            });
            let plan =
                crate::selections::reconcile_selected_nodes(&restore.selected, restore.previous.as_ref(), &snapshot);
            let confirm = crate::selections::selected_nodes_need_confirmation(&restore.selected, &snapshot);
            let mut attempted = false;
            for (group, node) in crate::selections::remaining_activations(&plan.activations, &restore.completed) {
                self.set_runtime_node(&group, Some(&node), deadline).await?;
                restore.completed.insert(group, node);
                attempted = true;
            }
            if attempted {
                snapshot = self.proxy_snapshot(deadline).await?;
            }
            if restore.prune && (!confirm || restore.previous.is_some()) && plan.repaired_count > 0 {
                self.profile_store
                    .save_selections(&restore.uid, plan.selected.clone())?;
                self.profile_state.send_replace(self.profile_store.snapshot());
                restore.selected = plan.selected;
            }
            let pending = crate::selections::unsettled_selections(&restore.selected, &snapshot)
                .into_iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>();
            restore.previous = Some(snapshot);
            Ok::<_, anyhow::Error>(pending)
        }
        .await;
        if *self.shutdown.borrow() {
            return;
        }
        let (pending, error) = match result {
            Ok(pending) => (pending, None),
            Err(error) => (
                restore
                    .selected
                    .iter()
                    .filter_map(|record| record.name.as_ref().map(ToString::to_string))
                    .collect(),
                Some(format!("node restoration failed: {error:#}")),
            ),
        };
        if pending.is_empty() && error.is_none() {
            self.status.send_modify(|state| {
                state.selection_pending.clear();
                state.selection_error = None;
            });
            return;
        }
        let expired = Instant::now() >= restore.deadline;
        let error = if expired {
            Some(format!(
                "node restoration deadline reached for [{}]{}",
                pending.join(", "),
                error.map(|error| format!(": {error}")).unwrap_or_default()
            ))
        } else {
            error
        };
        self.status.send_modify(|state| {
            state.selection_pending = if expired { Vec::new() } else { pending };
            state.selection_error = error.clone();
        });
        if expired {
            self.logs.append("manager", error.unwrap_or_default());
        } else {
            restore.next_at = (Instant::now() + self.options.policy.selection_interval).min(restore.deadline);
            self.restoration = Some(restore);
        }
    }
}
