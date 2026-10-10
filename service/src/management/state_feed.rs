//! Shared, demand-driven state observation for clients that do not need logs.
use super::Management;
use crate::core_manager::CorePhase;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, watch};

pub(super) struct StateFeed {
    latest: watch::Sender<Option<Value>>,
    started: AtomicBool,
    wake: Notify,
}

impl Default for StateFeed {
    fn default() -> Self {
        Self {
            latest: watch::channel(None).0,
            started: AtomicBool::new(false),
            wake: Notify::new(),
        }
    }
}

impl StateFeed {
    pub(super) fn subscribe(
        self: &Arc<Self>,
        management: Arc<Management>,
        closing: watch::Receiver<bool>,
    ) -> watch::Receiver<Option<Value>> {
        let mut receiver = self.latest.subscribe();
        // Each connection waits for a fresh observation, including reconnects
        // after the observer was idle. Never send the previous session's cache.
        receiver.borrow_and_update();
        if !self.started.swap(true, Ordering::SeqCst) {
            tokio::spawn(Arc::clone(self).run(management, closing));
        }
        self.wake.notify_one();
        receiver
    }

    async fn run(self: Arc<Self>, management: Arc<Management>, mut closing: watch::Receiver<bool>) {
        let mut status = management.manager.subscribe_status();
        let mut profiles = management.manager.subscribe_profiles();
        let mut preferences = management.preferences.subscribe();
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut force = false;
        loop {
            if *closing.borrow() {
                return;
            }
            if self.latest.receiver_count() == 0 {
                self.latest.send_replace(None);
                tokio::select! {
                    _ = closing.changed() => return,
                    _ = self.wake.notified() => {}
                }
                continue;
            }
            // Only the service observes core selections: Mihomo has no
            // selection-change stream. All state subscribers share this read.
            status.borrow_and_update();
            profiles.borrow_and_update();
            preferences.borrow_and_update();
            let current = tokio::select! {
                _ = closing.changed() => return,
                _ = self.latest.closed() => continue,
                _ = status.changed() => continue,
                _ = profiles.changed() => continue,
                _ = preferences.changed() => continue,
                current = observe(&management) => current,
            };
            // A new subscriber must get an initial state even if unchanged.
            // watch coalesces snapshots for slow subscribers instead of queuing.
            self.latest.send_if_modified(|latest| {
                let force = std::mem::take(&mut force);
                if !force && latest.as_ref() == Some(&current) {
                    false
                } else {
                    *latest = Some(current);
                    true
                }
            });
            tokio::select! {
                _ = closing.changed() => return,
                _ = self.latest.closed() => {},
                _ = self.wake.notified() => {
                    force = true;
                },
                changed = status.changed() => if changed.is_err() { return; },
                changed = profiles.changed() => if changed.is_err() { return; },
                changed = preferences.changed() => if changed.is_err() { return; },
                _ = interval.tick() => {},
            }
        }
    }
}

async fn observe(management: &Management) -> Value {
    let manager = &management.manager;
    let before = serde_json::to_value(manager.status()).expect("serializable core status");
    let running = manager.status().phase == CorePhase::Running;
    let (access, proxies) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(5), crate::proxy_access::inspect(manager)),
        async {
            if running {
                tokio::time::timeout(Duration::from_secs(3), manager.client().get_proxies())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .and_then(|value| serde_json::to_value(value).ok())
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        }
    );
    let status = serde_json::to_value(manager.status()).expect("serializable core status");
    let consistent = before == status;
    let profiles = serde_json::to_value(manager.profiles()).expect("serializable profiles");
    let mut user = serde_json::to_value(manager.multi_user()).expect("serializable multi-user facts");
    if let Some(user) = user.as_object_mut() {
        user.insert("tun_holder".into(), crate::proxy_access::tun_holder(manager));
    }
    json!({
        "status": status,
        "access": if consistent { access.ok().and_then(Result::ok).unwrap_or(Value::Null) } else { Value::Null },
        "proxies": if consistent { proxies } else { Value::Null },
        "profiles": profiles,
        "user": user,
        "preferences": management.preferences.get(),
    })
}
