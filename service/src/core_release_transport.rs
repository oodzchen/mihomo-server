//! Private route selection. No proxy endpoints/credentials enter release records or APIs.
use crate::{
    core_manager::{CoreStatus, same_proxy_snapshot},
    remote::{ManagedProxy, environment},
};
use anyhow::{Result, ensure};
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) enum Route {
    Managed {
        proxy: ManagedProxy,
        snapshot: Box<CoreStatus>,
        state: watch::Receiver<CoreStatus>,
    },
    System,
    Direct,
}
impl Route {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Managed { .. } => "managed",
            Self::System => "system",
            Self::Direct => "direct",
        }
    }
    pub(crate) fn configure(&self, builder: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder> {
        match self {
            Self::Managed { proxy, .. } => proxy.configure(builder),
            Self::Direct => Ok(builder.no_proxy()),
            Self::System => Ok(if environment::bypass_all()? {
                builder.no_proxy()
            } else {
                builder
            }),
        }
    }
    pub(crate) fn check(&self) -> Result<()> {
        if let Self::Managed { snapshot, state, .. } = self {
            ensure!(
                state.has_changed().is_ok(),
                "managed proxy changed during download; retry"
            );
            ensure!(
                same_proxy_snapshot(snapshot, &state.borrow()),
                "managed proxy changed during download; retry"
            );
        }
        Ok(())
    }
    pub(crate) async fn run<T>(&self, operation: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        self.check()?;
        if let Self::Managed { snapshot, state, .. } = self {
            let mut state = state.clone();
            let changed = async {
                loop {
                    if !same_proxy_snapshot(snapshot, &state.borrow_and_update()) {
                        break;
                    }
                    if state.changed().await.is_err() {
                        break;
                    }
                }
            };
            tokio::select! {biased;
                _ = changed => anyhow::bail!("managed proxy changed during download; retry"),
                result = operation => { self.check()?; result },
            }
        } else {
            operation.await
        }
    }
}
