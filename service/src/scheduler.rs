//! Adapted from pinned upstream core/timer.rs; bounded service tasks, no desktop singleton.
use super::{CommandMessage, CoreManager, CorePhase, CoreStatus, Logs};
use headless_core::config::{IProfiles, PrfItem};
use mihomo_client::Mihomo;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time::{Instant, sleep_until},
};

const MAX_RUNNING: usize = 4;

#[derive(Clone, PartialEq, Eq, Debug)]
struct Schedule {
    interval_minutes: u64,
    updated: Option<usize>,
    managed: bool,
}
impl Schedule {
    fn from_profile(item: &PrfItem) -> Option<Self> {
        if item.itype.as_deref() != Some("remote") || item.uid.is_none() || item.url.is_none() {
            return None;
        }
        let options = item.option.as_ref()?;
        let interval_minutes = options.update_interval?;
        if interval_minutes == 0 || options.allow_auto_update == Some(false) {
            return None;
        }
        Some(Self {
            interval_minutes,
            updated: item.updated,
            managed: options.self_proxy == Some(true),
        })
    }
    fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_minutes.saturating_mul(60))
    }
    fn first_delay(&self, now: u64) -> Duration {
        let elapsed = self.updated.filter(|updated| *updated > 0).map_or(0, |updated| {
            now.saturating_sub(u64::try_from(updated).unwrap_or(u64::MAX))
        });
        self.interval().saturating_sub(Duration::from_secs(elapsed))
    }
}
pub(super) fn eligible(item: &PrfItem) -> bool {
    Schedule::from_profile(item).is_some()
}
struct Task {
    schedule: Schedule,
    deadline: Option<Instant>,
    running: bool,
    retired: bool,
}
#[derive(Default)]
struct Tasks(HashMap<String, Task>);
impl Tasks {
    fn reconcile(&mut self, profiles: &IProfiles, now: Instant, wall: u64) {
        let schedules: HashMap<_, _> = profiles
            .items
            .iter()
            .flatten()
            .filter_map(|item| Some((item.uid.as_ref()?.to_string(), Schedule::from_profile(item)?)))
            .collect();
        self.0.retain(|uid, task| {
            if schedules.contains_key(uid) {
                return true;
            }
            task.deadline = None;
            task.retired = true;
            task.running // Keep the running guard across disable/re-enable.
        });
        for (uid, schedule) in schedules {
            if let Some(task) = self.0.get_mut(&uid) {
                let changed = task.retired || task.schedule != schedule;
                task.retired = false;
                if changed && !task.running {
                    task.deadline = now.checked_add(schedule.first_delay(wall));
                }
                task.schedule = schedule;
            } else {
                let deadline = now.checked_add(schedule.first_delay(wall));
                self.0.insert(
                    uid,
                    Task {
                        schedule,
                        deadline,
                        running: false,
                        retired: false,
                    },
                );
            }
        }
    }
    fn available(task: &Task, phase: CorePhase) -> bool {
        !task.running
            && !task.retired
            && !(task.schedule.managed
                && matches!(phase, CorePhase::Starting | CorePhase::Recovering | CorePhase::Stopping))
    }
    fn next(&self, phase: CorePhase) -> Option<(String, Instant)> {
        self.0
            .iter()
            .filter(|(_, task)| Self::available(task, phase))
            .filter_map(|(uid, task)| Some((uid.clone(), task.deadline?)))
            .min_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)))
    }
    fn begin(&mut self, uid: &str) {
        let task = self.0.get_mut(uid).unwrap();
        task.running = true;
        task.deadline = None;
    }
    fn finished(&mut self, uid: &str, now: Instant) {
        if let Some(task) = self.0.get_mut(uid) {
            if task.retired {
                self.0.remove(uid);
            } else {
                task.running = false;
                task.deadline = now.checked_add(task.schedule.interval());
            }
        }
    }
}
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// Holding a strong command sender in an idle background scheduler would prevent
// the actor from stopping when its last external manager is dropped.
pub(super) struct Access {
    commands: mpsc::WeakSender<CommandMessage>,
    remote_admission: Arc<tokio::sync::Semaphore>,
    state: watch::Receiver<CoreStatus>,
    shutdown: watch::Sender<bool>,
    completion: watch::Receiver<Option<std::result::Result<(), String>>>,
    scheduler_completion: watch::Receiver<bool>,
    logs: Logs,
    client: Arc<Mihomo>,
    profiles: watch::Receiver<IProfiles>,
}
impl Access {
    pub(super) fn new(manager: &CoreManager) -> Self {
        Self {
            commands: manager.commands.downgrade(),
            remote_admission: Arc::clone(&manager.remote_admission),
            state: manager.state.clone(),
            shutdown: manager.shutdown.clone(),
            completion: manager.completion.clone(),
            scheduler_completion: manager.scheduler_completion.clone(),
            logs: manager.logs.clone(),
            client: Arc::clone(&manager.client),
            profiles: manager.profiles.clone(),
        }
    }
    fn upgrade(&self) -> Option<CoreManager> {
        Some(CoreManager {
            commands: self.commands.upgrade()?,
            remote_admission: Arc::clone(&self.remote_admission),
            state: self.state.clone(),
            shutdown: self.shutdown.clone(),
            completion: self.completion.clone(),
            scheduler_completion: self.scheduler_completion.clone(),
            logs: self.logs.clone(),
            client: Arc::clone(&self.client),
            profiles: self.profiles.clone(),
        })
    }
}
pub(super) async fn run(access: Access) {
    let mut profiles = access.profiles.clone();
    let mut state = access.state.clone();
    let mut shutdown = access.shutdown.subscribe();
    let mut tasks = Tasks::default();
    let mut workers = JoinSet::<String>::new();
    loop {
        tasks.reconcile(&profiles.borrow_and_update(), Instant::now(), unix_now());
        let phase = state.borrow_and_update().phase;
        if *shutdown.borrow() || matches!(phase, CorePhase::Shutdown) {
            break;
        }
        let next = if workers.len() < MAX_RUNNING {
            tasks.next(phase)
        } else {
            None
        };
        let deadline = next.as_ref().map(|(_, deadline)| *deadline);
        tokio::select! {
            biased;
            _ = super::closing(&mut shutdown) => break,
            result = profiles.changed() => { if result.is_err() { break; } }
            result = state.changed() => { if result.is_err() { break; } }
            result = workers.join_next(), if !workers.is_empty() => {
                // A completed refresh may have published its new timestamp before
                // this branch was selected. Reconcile before re-arming the task.
                tasks.reconcile(&profiles.borrow_and_update(), Instant::now(), unix_now());
                match result {
                    Some(Ok(uid)) => tasks.finished(&uid, Instant::now()),
                    Some(Err(_)) => { access.logs.append("scheduler", "Scheduled subscription worker failed; scheduler stopped".into()); break; }
                    None => {}
                }
            }
            _ = async { if let Some(deadline) = deadline { sleep_until(deadline).await } else { std::future::pending().await } } => {
                let Some((uid, _)) = next else { continue; };
                let Some(manager) = access.upgrade() else { break; };
                tasks.begin(&uid);
                workers.spawn(async move {
                    manager.logs.append("scheduler", "Scheduled subscription refresh started".into());
                    let result = manager.refresh_profile_mode(uid.clone(), true).await;
                    // Don't echo provider bodies, private URLs or credentials in logs.
                    let message = if result.is_ok() { "Scheduled subscription refresh completed" } else { "Scheduled subscription refresh failed; retry after saved interval" };
                    manager.logs.append("scheduler", message.into());
                    uid
                });
            }
        }
    }
    workers.abort_all();
    while workers.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use headless_core::config::PrfOption;
    fn profile(interval: u64, updated: Option<usize>) -> PrfItem {
        PrfItem {
            uid: Some("scheduled".into()),
            itype: Some("remote".into()),
            url: Some("https://provider.invalid/".into()),
            updated,
            option: Some(PrfOption {
                update_interval: Some(interval),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    fn catalog(item: PrfItem) -> IProfiles {
        IProfiles {
            items: Some(vec![item]),
            ..Default::default()
        }
    }
    #[test]
    fn first_due_matches_upstream_updated_rules_and_saturates_large_intervals() {
        for (updated, wall, expected) in [
            (None, 1000, 120),
            (Some(0), 1000, 120),
            (Some(950), 1000, 70),
            (Some(1100), 1000, 120),
            (Some(1), 1000, 0),
        ] {
            assert_eq!(
                Schedule::from_profile(&profile(2, updated)).unwrap().first_delay(wall),
                Duration::from_secs(expected)
            );
        }
        assert_eq!(
            Schedule::from_profile(&profile(u64::MAX, Some(1)))
                .unwrap()
                .interval()
                .as_secs(),
            u64::MAX
        );
    }
    #[test]
    fn only_remote_positive_allowed_schedules_with_uid_and_url_are_registered() {
        let mut item = profile(1, Some(1));
        assert!(eligible(&item));
        item.option.as_mut().unwrap().allow_auto_update = Some(false);
        assert!(!eligible(&item));
        item.option.as_mut().unwrap().allow_auto_update = Some(true);
        assert!(eligible(&item));
        item.option.as_mut().unwrap().update_interval = Some(0);
        assert!(!eligible(&item));
        item.option.as_mut().unwrap().update_interval = Some(1);
        item.itype = Some("local".into());
        assert!(!eligible(&item));
        item.itype = Some("remote".into());
        item.url = None;
        assert!(!eligible(&item));
    }
    #[test]
    fn failure_rearms_full_interval_and_unrelated_updates_do_not_spin_or_postpone() {
        let start = Instant::now();
        let mut tasks = Tasks::default();
        let profiles = catalog(profile(1, Some(1)));
        tasks.reconcile(&profiles, start, 1000);
        assert_eq!(tasks.next(CorePhase::Stopped).unwrap().1, start);
        tasks.begin("scheduled");
        assert!(tasks.next(CorePhase::Stopped).is_none());
        tasks.finished("scheduled", start);
        tasks.reconcile(&profiles, start + Duration::from_secs(1), 1001);
        assert_eq!(
            tasks.next(CorePhase::Stopped).unwrap().1,
            start + Duration::from_secs(60)
        );
        // A successful manual refresh changes updated and resets the first deadline.
        tasks.reconcile(&catalog(profile(1, Some(1010))), start + Duration::from_secs(10), 1010);
        assert_eq!(
            tasks.next(CorePhase::Stopped).unwrap().1,
            start + Duration::from_secs(70)
        );
    }
    #[test]
    fn disable_reenable_and_interval_edits_keep_running_guard_and_retire_once_finished() {
        let start = Instant::now();
        let mut tasks = Tasks::default();
        tasks.reconcile(&catalog(profile(1, Some(1))), start, 1000);
        tasks.begin("scheduled");
        tasks.reconcile(&IProfiles::default(), start, 1000);
        assert!(tasks.0["scheduled"].retired);
        tasks.reconcile(&catalog(profile(2, Some(1))), start, 1000);
        assert!(tasks.next(CorePhase::Stopped).is_none());
        tasks.finished("scheduled", start);
        assert_eq!(
            tasks.next(CorePhase::Stopped).unwrap().1,
            start + Duration::from_secs(120)
        );
        tasks.begin("scheduled");
        tasks.reconcile(&IProfiles::default(), start, 1000);
        tasks.finished("scheduled", start);
        assert!(tasks.0.is_empty());
    }
    #[test]
    fn managed_startup_waits_for_stable_core_and_overflow_never_panics() {
        let start = Instant::now();
        let mut item = profile(1, Some(1));
        item.option.as_mut().unwrap().self_proxy = Some(true);
        let mut tasks = Tasks::default();
        tasks.reconcile(&catalog(item), start, 1000);
        assert!(tasks.next(CorePhase::Starting).is_none());
        assert!(tasks.next(CorePhase::Recovering).is_none());
        assert!(tasks.next(CorePhase::Running).is_some());
        tasks.reconcile(&catalog(profile(u64::MAX, Some(1))), start, 1000);
        assert!(tasks.next(CorePhase::Stopped).is_none());
    }
}
