import { useEffect, useState } from "react";
import { command, type Connection } from "./api";
import { t, type Language } from "./i18n";

type MultiUser = {
  uid: number;
  slot: number;
  mixed_port: number;
  dns_listen: string;
  tun_device: string;
  tun_capable: boolean;
};

// Shown only when the service runs from a shared multi-user installation.
export function MultiUserPanel({ token, language, connection }: {
  token: string;
  language: Language;
  connection: Connection;
}) {
  const [value, setValue] = useState<MultiUser | null>(null);
  useEffect(() => {
    if (connection !== "connected") return;
    const controller = new AbortController();
    command<MultiUser | null>(token, "multi_user", {}, controller.signal)
      .then(setValue)
      .catch(() => setValue(null));
    return () => controller.abort();
  }, [token, connection]);
  if (!value) return null;
  return (
    <section className="panel" aria-label={t(language, "multiUserTitle")}>
      <div className="panel-title">
        <h2>{t(language, "multiUserTitle")}</h2>
      </div>
      <p className="muted">{t(language, "multiUserHint")}</p>
      <dl>
        <div>
          <dt>{t(language, "multiUserSlot")}</dt>
          <dd className="mono">{value.slot} (uid {value.uid})</dd>
        </div>
        <div>
          <dt>{t(language, "multiUserTunDevice")}</dt>
          <dd className="mono">{value.tun_device}</dd>
        </div>
        <div>
          <dt>{t(language, "multiUserTun")}</dt>
          <dd>
            <span className={`badge ${value.tun_capable ? "good" : ""}`}>
              {t(language, value.tun_capable ? "multiUserTunAvailable" : "multiUserTunUnavailable")}
            </span>
          </dd>
        </div>
      </dl>
    </section>
  );
}
