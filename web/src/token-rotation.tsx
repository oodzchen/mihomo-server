import { useEffect, useState } from "react";
import { ApiError, type TokenRotation } from "./api";
import { ConfirmDialog } from "./confirm-dialog";
import { describe } from "./format";
import { HelpTip } from "./help-tip";
import { t, type Language } from "./i18n";
import { useToast } from "./toast";

/** Periods offered besides the instance's current one (days; null: never). */
const PERIODS = [7, 30, 90, 180, 365];
const REMINDED_KEY = "mihomo.tokenReminder";

function locale(language: Language) {
  return language === "en" ? "en-US" : language === "zhtw" ? "zh-TW" : "zh-CN";
}

export function rotationDate(seconds: number, language: Language) {
  return new Date(seconds * 1000).toLocaleString(locale(language), { hour12: false, dateStyle: "medium", timeStyle: "short" });
}

export function rotationRemaining(seconds: number, language: Language, now = Date.now()) {
  const left = seconds * 1000 - now;
  const format = new Intl.RelativeTimeFormat(locale(language), { numeric: "auto" });
  const hours = Math.max(0, Math.round(left / 3_600_000));
  return hours >= 24 ? format.format(Math.round(hours / 24), "day") : format.format(hours, "hour");
}

/** Whether ROTATION is inside its reminder window. */
export function rotationSoon(rotation: TokenRotation | undefined, now = Date.now()) {
  return !!rotation?.remind_at && !!rotation.expires_at
    && now >= rotation.remind_at * 1000 && now < rotation.expires_at * 1000;
}

/**
 * Remind once per browser session (and per schedule) when the token is about
 * to rotate: a toast, so nothing in the page moves.
 */
export function useTokenReminder(rotation: TokenRotation | undefined, language: Language) {
  const notify = useToast();
  useEffect(() => {
    if (!rotation?.remind_at || !rotation.expires_at) return;
    const { remind_at: remindAt, expires_at: expiresAt } = rotation;
    function remind() {
      try {
        if (window.sessionStorage.getItem(REMINDED_KEY) === String(expiresAt)) return;
        window.sessionStorage.setItem(REMINDED_KEY, String(expiresAt));
      } catch { /* Without storage the reminder can repeat on reload. */ }
      notify(t(language, "tokenReminder", {
        date: rotationDate(expiresAt, language),
        remaining: rotationRemaining(expiresAt, language),
      }), "info");
    }
    const delay = remindAt * 1000 - Date.now();
    if (Date.now() >= expiresAt * 1000) return;
    if (delay <= 0) {
      remind();
      return;
    }
    // Longer delays overflow timers; a reconnect reads the schedule again.
    if (delay > 2 ** 31 - 1) return;
    const timer = setTimeout(remind, delay);
    return () => clearTimeout(timer);
  }, [rotation, language, notify]);
}

/** Rotation period, schedule and an immediate rotation for the management token. */
export function TokenPanel({ rotation, language, disabled, change, logout }: {
  rotation: TokenRotation | undefined;
  language: Language;
  disabled: boolean;
  /** Runs a token command; a returned token already replaced this page's. */
  change: (command: "rotate_token" | "set_token_rotation", fields?: Record<string, unknown>) => Promise<TokenRotation>;
  logout: (reason?: string) => void;
}) {
  const notify = useToast();
  const [working, setWorking] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [, setClock] = useState(0);
  // Keep "in N days" current while the page stays open.
  useEffect(() => {
    const timer = setInterval(() => setClock(value => value + 1), 60_000);
    return () => clearInterval(timer);
  }, []);
  const periods = rotation?.period_days && !PERIODS.includes(rotation.period_days)
    ? [...PERIODS, rotation.period_days].sort((a, b) => a - b)
    : PERIODS;

  async function run(command: "rotate_token" | "set_token_rotation", fields?: Record<string, unknown>) {
    setWorking(true);
    try {
      const result = await change(command, fields);
      notify(t(language, result.token
        ? command === "rotate_token" ? "tokenRotatedHere" : "tokenPeriodRotated"
        : "saveSuccess"));
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
      else notify(describe(error), "error");
    } finally {
      setWorking(false);
    }
  }

  const busy = disabled || working || !rotation;
  return (
    <section className="panel token-panel" aria-label={t(language, "tokenTitle")}>
      <div className="panel-title">
        <h2 className="setting-heading">
          {t(language, "tokenTitle")}
          <HelpTip label={t(language, "tokenTitle")}>{t(language, "tokenHelp")}</HelpTip>
        </h2>
      </div>
      <label className="language-picker">
        {t(language, "tokenPeriod")}
        <select
          value={rotation ? String(rotation.period_days ?? "never") : ""}
          disabled={busy}
          onChange={event => void run("set_token_rotation", {
            period_days: event.target.value === "never" ? null : Number(event.target.value),
          })}
        >
          {!rotation && <option value="">—</option>}
          {periods.map(days => (
            <option key={days} value={days}>{t(language, "tokenPeriodDays", { days })}</option>
          ))}
          <option value="never">{t(language, "tokenPeriodNever")}</option>
        </select>
      </label>
      <dl className="token-schedule">
        <dt>{t(language, "tokenIssued")}</dt>
        <dd>{rotation ? rotationDate(rotation.issued_at, language) : "—"}</dd>
        <dt>{t(language, "tokenNextRotation")}</dt>
        <dd>
          {!rotation ? "—" : rotation.expires_at
            ? `${rotationDate(rotation.expires_at, language)}${t(language, "parenthetical", { text: rotationRemaining(rotation.expires_at, language) })}`
            : t(language, "tokenNoRotation")}
          {rotationSoon(rotation) && <span className="token-soon">{t(language, "tokenSoon")}</span>}
        </dd>
      </dl>
      <button type="button" disabled={busy} onClick={() => setConfirming(true)}>
        {t(language, "tokenRotateNow")}
      </button>
      {confirming && (
        <ConfirmDialog
          title={t(language, "tokenRotateTitle")}
          confirmLabel={t(language, "tokenRotateNow")}
          cancelLabel={t(language, "cancel")}
          confirmDisabled={busy}
          onConfirm={() => { setConfirming(false); void run("rotate_token"); }}
          onCancel={() => setConfirming(false)}
        >
          <p>{t(language, "tokenRotateWarning")}</p>
        </ConfirmDialog>
      )}
    </section>
  );
}
