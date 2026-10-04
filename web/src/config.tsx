import { useToast } from "./toast";
import { useEffect, useState, type FormEvent } from "react";
import { command, type Perform } from "./api";
import { t, type Language } from "./i18n";
import type { CoreStatus } from "./types";
import { describe } from "./format";

export function ConfigPage({
  token,
  language,
  busy,
  perform,
}: {
  token: string;
  language: Language;
  busy: boolean;
  perform: Perform;
}) {
  const notify = useToast();
  const [yaml, setYaml] = useState(""),
    [loading, setLoading] = useState(true),
    [message, setMessage] = useState<
      { kind: "missing" } | { kind: "error"; detail: string } | null
    >(null),
    [dirty, setDirty] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    command<{ yaml: string }>(token, "config", {}, controller.signal)
      .then((value) => {
        setYaml(value.yaml);
        setLoading(false);
      })
      .catch((error) => {
        if (!controller.signal.aborted) {
          setLoading(false);
          const detail = describe(error);
          setMessage(detail.includes("no committed configuration")
            ? { kind: "missing" }
            : { kind: "error", detail });
        }
      });
    return () => controller.abort();
  }, [token]);
  async function apply(event: FormEvent) {
    event.preventDefault();
    const toast = notify.loading(t(language, "working"));
    const result = await perform<CoreStatus>("edit_config", { yaml }, { notify: false, toast });
    if (result) {
      setDirty(false);
      setMessage(null);
      toast.finish(t(language, "configApplied"));
    }
  }
  return (
    <section className="panel">
      <div className="panel-title">
        <h2>{t(language, "configTitle")}</h2>
        <span>{dirty ? t(language, "configDirty") : t(language, "configCompleteYaml")}</span>
      </div>
      <p className="muted">
        {t(language, "configDescription")}
      </p>
      {message && <p className="info">
        {message.kind === "missing" ? t(language, "configMissing") :
          message.detail}
      </p>}
      <form onSubmit={apply}>
        <label>
          {t(language, "configYamlLabel")}
          <textarea
            className="code editor"
            value={yaml}
            onChange={(event) => {
              setYaml(event.target.value);
              setDirty(true);
            }}
            disabled={loading || busy}
            required
            spellCheck={false}
          />
        </label>
        <div className="form-actions">
          <p className="hint">{t(language, "configLeaveHint")}</p>
          <button
            className="primary"
            disabled={loading || busy || !yaml.trim()}
          >
            {t(language, "configApply")}
          </button>
        </div>
      </form>
    </section>
  );
}
