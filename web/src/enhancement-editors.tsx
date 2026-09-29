import { useRef, useState, type FormEvent } from "react";
import type { Perform } from "./api";
import { t, type Language } from "./i18n";
import type { CoreStatus, Profile } from "./types";

export function MergeEditor({
  item,
  language,
  content,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
  language: Language;
  content: { uid?: string; yaml?: string };
  busy: boolean;
  perform: Perform;
  onClose: () => void;
}) {
  const [yaml, setYaml] = useState(content.yaml ?? "{}\n");
  async function save(event: FormEvent) {
    event.preventDefault();
    if (await perform<Profile>("set_profile_merge", { uid: item.uid, yaml }))
      onClose();
  }
  async function clear() {
    if (await perform<Profile>("clear_profile_merge", { uid: item.uid }))
      onClose();
  }
  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <section className="modal-dialog modal-dialog-lg" aria-label={t(language, "mergeEditorTitle")}>
        <div className="modal-header">
          <h2>{t(language, "mergeEditorTitle")}</h2>
          <button
            type="button"
            className="modal-close"
            onClick={onClose}
            aria-label={t(language, "close")}
          >
            ✕
          </button>
        </div>
        <div className="modal-body">
          <p className="muted">{t(language, "mergeEditorHelp").replace("{name}", item.name || item.uid)}</p>
          <form onSubmit={save}>
            <label>
              {t(language, "mergeEditorYaml")}
              <textarea
                className="code"
                rows={12}
                disabled={busy}
                value={yaml}
                onChange={(event) => setYaml(event.target.value)}
                spellCheck={false}
                required
              />
            </label>
            <div className="form-actions">
              <button className="primary" disabled={busy}>
                {t(language, "mergeEditorSave")}
              </button>
              <button
                type="button"
                disabled={busy || !content.uid}
                onClick={() => void clear()}
              >
                {t(language, "mergeEditorRemove")}
              </button>
              <button type="button" disabled={busy} onClick={onClose}>
                {t(language, "mergeEditorCancel")}
              </button>
            </div>
          </form>
        </div>
      </section>
    </div>
  );
}

export type SequenceKind = "rules" | "proxies" | "groups";

export type GlobalKind = "merge" | "script";

export function GlobalEnhancements({
  language,
  status,
  busy,
  perform,
}: {
  language: Language;
  status: CoreStatus;
  busy: boolean;
  perform: Perform;
}) {
  const [editing, setEditing] = useState<{
    kind: GlobalKind;
    content: string;
    loaded: boolean;
    version: number;
  } | null>(null);
  const version = useRef(0);
  async function open(kind: GlobalKind) {
    const result = await perform<{ yaml?: string; source?: string }>(
      `global_${kind}`,
    );
    const content = kind === "merge" ? result?.yaml : result?.source;
    setEditing({
      kind,
      content: content ?? "",
      loaded: typeof content === "string",
      version: ++version.current,
    });
  }
  return (
    <section className="panel global-enhancements" aria-label={t(language, "globalPanelTitle")}>
      <div className="panel-title">
        <h2>{t(language, "globalPanelTitle")}</h2>
        <div className="actions">
          <button
            disabled={busy || editing !== null}
            onClick={() => void open("merge")}
          >
            {t(language, "globalMergeOpen")}
          </button>
          <button
            disabled={busy || editing !== null}
            onClick={() => void open("script")}
          >
            {t(language, "globalScriptOpen")}
          </button>
        </div>
      </div>
      <p className="muted">{t(language, "globalPanelHelp")}</p>
      <p className="hint">
        {status.active_profile
          ? t(language, "globalPanelActive")
          : t(language, "globalPanelInactive")}
      </p>
      {editing && (
        <GlobalEditor
          key={editing.version}
          language={language}
          kind={editing.kind}
          initial={editing.content}
          loaded={editing.loaded}
          busy={busy}
          perform={perform}
          onRetry={() => void open(editing.kind)}
          onClose={() => setEditing(null)}
        />
      )}
    </section>
  );
}

export function GlobalEditor({
  language,
  kind,
  initial,
  loaded,
  busy,
  perform,
  onRetry,
  onClose,
}: {
  language: Language;
  kind: GlobalKind;
  initial: string;
  loaded: boolean;
  busy: boolean;
  perform: Perform;
  onRetry: () => void;
  onClose: () => void;
}) {
  const [content, setContent] = useState(initial);
  const [resetting, setResetting] = useState(false);
  const [error, setError] = useState<"script-large" | "merge-large" | null>(null);
  const script = kind === "script";
  const title = t(language, script ? "globalScriptTitle" : "globalMergeTitle");
  async function save(event: FormEvent) {
    event.preventDefault();
    setError(null);
    const limit = (script ? 1 : 8) * 1024 ** 2;
    if (new TextEncoder().encode(content).length > limit) {
      setError(script ? "script-large" : "merge-large");
      return;
    }
    if (
      await perform<Profile>(
        `set_global_${kind}`,
        script ? { source: content } : { yaml: content },
      )
    )
      onClose();
  }
  async function reset() {
    setError(null);
    if (await perform<Profile>(`reset_global_${kind}`)) onClose();
  }
  return (
    <form className="global-editor" onSubmit={save} aria-label={title}>
      <h3>{title}</h3>
      <p className="muted">
        {script
          ? t(language, "globalScriptHelp")
          : t(language, "globalMergeHelp")}
      </p>
      {!loaded && (
        <p className="info">
          {t(language, "globalReadMissing")}
        </p>
      )}
      <label>
        {t(language, script ? "globalScriptSource" : "globalMergeYaml")}
        <textarea
          className="code"
          rows={12}
          value={content}
          disabled={busy}
          onChange={(event) => {
            setContent(event.target.value);
            setError(null);
            setResetting(false);
          }}
          spellCheck={false}
          required
        />
      </label>
      {error && (
        <p className="alert" role="alert">
          {t(language, error === "script-large" ? "globalScriptTooLarge" : "globalMergeTooLarge")}
        </p>
      )}
      <div className="actions">
        <button className="primary" disabled={busy || !content.trim()}>
          {t(language, script ? "globalScriptSave" : "globalMergeSave")}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => setResetting(true)}
        >
          {t(language, script ? "globalScriptReset" : "globalMergeReset")}
        </button>
        {!loaded && !content && (
          <button type="button" disabled={busy} onClick={onRetry}>
            {t(language, "globalRetry")}
          </button>
        )}
        <button type="button" disabled={busy} onClick={onClose}>
          {t(language, "globalCancel")}
        </button>
      </div>
      {resetting && (
        <div
          className="reset-confirmation"
          role="group"
          aria-label={t(language, "globalResetConfirmRegion")}
        >
          <p className="muted">
            {script
              ? t(language, "globalScriptResetWarning")
              : t(language, "globalMergeResetWarning")}
          </p>
          <div className="actions">
            <button type="button" disabled={busy} onClick={() => void reset()}>
              {t(language, "globalResetConfirm")}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => setResetting(false)}
            >
              {t(language, "globalKeepEditing")}
            </button>
          </div>
        </div>
      )}
    </form>
  );
}

export function ScriptEditor({
  item,
  language,
  content,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
  language: Language;
  content: { uid?: string; source?: string };
  busy: boolean;
  perform: Perform;
  onClose: () => void;
}) {
  const [source, setSource] = useState(
    content.source ?? "function main(config, name) {\n  return config;\n}\n",
  );
  async function save(event: FormEvent) {
    event.preventDefault();
    if (await perform<Profile>("set_profile_script", { uid: item.uid, source }))
      onClose();
  }
  async function clear() {
    if (await perform<Profile>("clear_profile_script", { uid: item.uid }))
      onClose();
  }
  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <section className="modal-dialog modal-dialog-lg" aria-label={t(language, "scriptEditorTitle")}>
        <div className="modal-header">
          <h2>{t(language, "scriptEditorTitle")}</h2>
          <button
            type="button"
            className="modal-close"
            onClick={onClose}
            aria-label={t(language, "close")}
          >
            ✕
          </button>
        </div>
        <div className="modal-body">
          <p className="muted">{t(language, "scriptEditorHelp").replace("{name}", item.name || item.uid)}</p>
          <form onSubmit={save}>
            <label>
              {t(language, "scriptEditorSource")}
              <textarea
                className="code"
                rows={12}
                disabled={busy}
                value={source}
                onChange={(event) => setSource(event.target.value)}
                spellCheck={false}
                required
              />
            </label>
            <div className="form-actions">
              <button className="primary" disabled={busy}>
                {t(language, "scriptEditorSave")}
              </button>
              <button
                type="button"
                disabled={busy || !content.uid}
                onClick={() => void clear()}
              >
                {t(language, "scriptEditorRemove")}
              </button>
              <button type="button" disabled={busy} onClick={onClose}>
                {t(language, "scriptEditorCancel")}
              </button>
            </div>
          </form>
        </div>
      </section>
    </div>
  );
}

export function SequenceEditor({
  item,
  language,
  kind,
  content,
  busy,
  perform,
  onKind,
  onClose,
}: {
  item: Profile;
  language: Language;
  kind: SequenceKind;
  content: { uid?: string; yaml?: string };
  busy: boolean;
  perform: Perform;
  onKind: (kind: SequenceKind) => void;
  onClose: () => void;
}) {
  const [yaml, setYaml] = useState(
    content.yaml ?? "prepend: []\nappend: []\ndelete: []\n",
  );
  async function save(event: FormEvent) {
    event.preventDefault();
    if (
      await perform<Profile>("set_profile_sequence", {
        uid: item.uid,
        kind,
        yaml,
      })
    )
      onClose();
  }
  async function clear() {
    if (
      await perform<Profile>("clear_profile_sequence", { uid: item.uid, kind })
    )
      onClose();
  }
  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <section className="modal-dialog modal-dialog-lg" aria-label={t(language, "sequenceEditorTitle")}>
        <div className="modal-header">
          <h2>{t(language, "sequenceEditorTitle")}</h2>
          <button
            type="button"
            className="modal-close"
            onClick={onClose}
            aria-label={t(language, "close")}
          >
            ✕
          </button>
        </div>
        <div className="modal-body">
          <p className="muted">{t(language, "sequenceEditorHelp").replace("{name}", item.name || item.uid)}</p>
          <label>
            {t(language, "sequenceEditorKind")}
            <select
              value={kind}
              disabled={busy}
              onChange={(event) => onKind(event.target.value as SequenceKind)}
            >
              <option value="rules">{t(language, "sequenceRules")}</option>
              <option value="proxies">{t(language, "sequenceProxies")}</option>
              <option value="groups">{t(language, "sequenceGroups")}</option>
            </select>
          </label>
          <form onSubmit={save}>
            <label>
              {t(language, "sequenceEditorYaml")}
              <textarea
                className="code"
                rows={12}
                disabled={busy}
                value={yaml}
                onChange={(event) => setYaml(event.target.value)}
                spellCheck={false}
                required
              />
            </label>
            <div className="form-actions">
              <button className="primary" disabled={busy}>
                {t(language, "sequenceEditorSave")}
              </button>
              <button
                type="button"
                disabled={busy || !content.uid}
                onClick={() => void clear()}
              >
                {t(language, "sequenceEditorRemove")}
              </button>
              <button type="button" disabled={busy} onClick={onClose}>
                {t(language, "sequenceEditorCancel")}
              </button>
            </div>
          </form>
        </div>
      </section>
    </div>
  );
}
