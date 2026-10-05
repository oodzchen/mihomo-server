import { useEffect, useState } from "react";
import { ApiError, command, type Connection } from "./api";
import { describe } from "./format";
import { HelpTip } from "./help-tip";
import { t, type Language } from "./i18n";
import { useToast } from "./toast";

/** `undefined` while reading; `null` when it cannot be set. */
type Switch = boolean | null | undefined;

function Toggle({ label, value, working, hint, onChange }: {
  label: string;
  value: Switch;
  working: boolean;
  hint?: string;
  onChange: (enabled: boolean) => void;
}) {
  return (
    <div className="autostart-row">
      <span className="setting-name">
        {label}
        {value === null && hint && <HelpTip label={label}>{hint}</HelpTip>}
      </span>
      <button
        type="button"
        role="switch"
        className="switch"
        aria-label={label}
        aria-checked={value === true}
        aria-busy={working}
        disabled={value == null || working}
        onClick={() => onChange(!value)}
      >
        <span />
      </button>
    </div>
  );
}

/**
 * Start at login for the service (its systemd unit) and, inside the desktop
 * client only, for the client itself (through the client's own commands).
 */
export function AutostartPanel({ token, language, connection, logout }: {
  token: string;
  language: Language;
  connection: Connection;
  logout: (reason?: string) => void;
}) {
  const notify = useToast();
  const desktop = window.__MIHOMO_DESKTOP_VERSION__ ? window.__TAURI_INTERNALS__ : undefined;
  const [service, setService] = useState<Switch>();
  const [client, setClient] = useState<Switch>();
  const [working, setWorking] = useState<"service" | "client">();

  useEffect(() => {
    if (connection !== "connected") return;
    const controller = new AbortController();
    command<{ autostart: boolean | null }>(token, "service_info", {}, controller.signal)
      .then(info => setService(info.autostart))
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
        else setService(null);
      });
    return () => controller.abort();
  }, [token, connection, logout, language]);

  useEffect(() => {
    if (!desktop) return;
    desktop.invoke<boolean>("client_autostart").then(setClient, () => setClient(null));
  }, [desktop]);

  async function change(target: "service" | "client", enabled: boolean) {
    setWorking(target);
    try {
      if (target === "service")
        setService(await command<boolean>(token, "set_service_autostart", { enabled }));
      else setClient(await desktop!.invoke<boolean>("set_client_autostart", { enabled }));
      notify(t(language, "saveSuccess"));
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
      else notify(describe(error), "error");
    } finally {
      setWorking(undefined);
    }
  }

  return (
    <section className="panel" aria-label={t(language, "autostartTitle")}>
      <div className="panel-title">
        <h2 className="setting-heading">
          {t(language, "autostartTitle")}
          <HelpTip label={t(language, "autostartTitle")}>{t(language, "autostartHelp")}</HelpTip>
        </h2>
      </div>
      <Toggle
        label={t(language, "autostartService")}
        value={service}
        working={working === "service" || connection !== "connected"}
        hint={t(language, "autostartServiceUnavailable")}
        onChange={enabled => void change("service", enabled)}
      />
      {desktop && (
        <Toggle
          label={t(language, "autostartClient")}
          value={client}
          working={working === "client"}
          onChange={enabled => void change("client", enabled)}
        />
      )}
    </section>
  );
}
