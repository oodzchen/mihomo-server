//! Desktop notifications (freedesktop D-Bus) for failures: the tray menu stays
//! short and Linux trays show no tooltip, so errors would otherwise be unseen.
use std::collections::HashMap;
use zbus::zvariant::Value;

pub async fn failure(summary: &str, body: &str) {
    let hints: HashMap<&str, Value<'_>> = HashMap::new();
    let sent = async {
        zbus::Connection::session()
            .await?
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                // app name, replaces id, icon, summary, body, actions, hints, timeout
                &(
                    "Mihomo Server",
                    0u32,
                    "mihomo-server-desktop",
                    summary,
                    body,
                    Vec::<&str>::new(),
                    hints,
                    -1i32,
                ),
            )
            .await
    };
    if let Err(error) = sent.await {
        eprintln!("cannot show a notification: {error}");
    }
}
