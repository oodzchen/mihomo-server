import { useEffect, useRef, useState, type FormEvent } from "react";
import type { Perform } from "./api";
import { RawEditor } from "./raw-editor";
import { t, type Language } from "./i18n";
import type { CoreStatus, Profile, Profiles } from "./types";
import { bytes, describe } from "./format";
import { MergeEditor, type SequenceKind, GlobalEnhancements, ScriptEditor, SequenceEditor } from "./enhancement-editors";

export function ProfileEditor({
  item,
  language,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
  language: Language;
  busy: boolean;
  perform: Perform;
  onClose: () => void;
}) {
  const [name, setName] = useState(item.name || "");
  const [desc, setDesc] = useState(item.desc || "");
  const [url, setUrl] = useState(item.url || "");
  const [agent, setAgent] = useState(item.option?.user_agent || "");
  const [seconds, setSeconds] = useState(
    String(item.option?.timeout_seconds ?? 20),
  );
  const [interval, setInterval] = useState(
    String(item.option?.update_interval ?? 0),
  );
  const [auto, setAuto] = useState(item.option?.allow_auto_update ?? true);
  const [selfProxy, setSelfProxy] = useState(item.option?.self_proxy ?? false);
  const [withProxy, setWithProxy] = useState(item.option?.with_proxy ?? false);
  const [invalidCerts, setInvalidCerts] = useState(
    item.option?.danger_accept_invalid_certs ?? false,
  );
  async function save(event: FormEvent) {
    event.preventDefault();
    const patch: Record<string, unknown> = {};
    if (name !== (item.name || "")) patch.name = name;
    if (desc !== (item.desc || "")) patch.desc = desc;
    if (item.type === "remote") {
      if (url !== (item.url || "")) patch.url = url;
      const options: Record<string, unknown> = {};
      if (agent !== (item.option?.user_agent || "")) options.user_agent = agent;
      if (seconds !== String(item.option?.timeout_seconds ?? 20))
        options.timeout_seconds = Number(seconds);
      if (interval !== String(item.option?.update_interval ?? 0))
        options.update_interval = Number(interval);
      if (auto !== (item.option?.allow_auto_update ?? true))
        options.allow_auto_update = auto;
      if (selfProxy !== (item.option?.self_proxy ?? false))
        options.self_proxy = selfProxy;
      if (withProxy !== (item.option?.with_proxy ?? false))
        options.with_proxy = withProxy;
      if (invalidCerts !== (item.option?.danger_accept_invalid_certs ?? false))
        options.danger_accept_invalid_certs = invalidCerts;
      if (Object.keys(options).length) patch.options = options;
    }
    if (!Object.keys(patch).length) {
      onClose();
      return;
    }
    if (await perform<Profile>("edit_profile", { uid: item.uid, patch }))
      onClose();
  }
  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <section className="modal-dialog" aria-label={t(language, "profileEditorTitle")}>
        <div className="modal-header">
          <h2>{t(language, "profileEditorTitle")}</h2>
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
          <p className="muted">{t(language, "profileEditorHelp")}</p>
          <form onSubmit={save}>
            <label>
              {t(language, "profileEditorName")}
              <input
                required
                maxLength={256}
                disabled={busy}
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </label>
            <label>
              {t(language, "profileEditorDescription")}
              <textarea
                maxLength={4096}
                disabled={busy}
                value={desc}
                onChange={(event) => setDesc(event.target.value)}
              />
            </label>
            {item.type === "remote" && (
              <>
                <label>
                  {t(language, "profileEditorUrl")}
                  <input
                    type="url"
                    required
                    maxLength={8192}
                    disabled={busy}
                    value={url}
                    onChange={(event) => setUrl(event.target.value)}
                  />
                </label>
                <label>
                  {t(language, "profileEditorAgent")}
                  <input
                    maxLength={1024}
                    disabled={busy}
                    value={agent}
                    onChange={(event) => setAgent(event.target.value)}
                  />
                </label>
                <label>
                  {t(language, "profileEditorTimeout")}
                  <input
                    type="number"
                    required
                    min={1}
                    max={120}
                    step={1}
                    disabled={busy}
                    value={seconds}
                    onChange={(event) => setSeconds(event.target.value)}
                  />
                </label>
                <label>
                  {t(language, "profileEditorInterval")}
                  <input
                    type="number"
                    required
                    min={0}
                    max={Number.MAX_SAFE_INTEGER}
                    step={1}
                    disabled={busy}
                    value={interval}
                    onChange={(event) => setInterval(event.target.value)}
                  />
                </label>
                <label className="check-label">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={auto}
                    onChange={(event) => setAuto(event.target.checked)}
                  />
                  {t(language, "profileEditorAuto")}
                </label>
                <label className="check-label">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={selfProxy}
                    onChange={(event) => setSelfProxy(event.target.checked)}
                  />
                  {t(language, "profileEditorManaged")}
                </label>
                <label className="check-label">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={withProxy}
                    onChange={(event) => setWithProxy(event.target.checked)}
                  />
                  {t(language, "profileEditorSystem")}
                </label>
                <label className="check-label">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={invalidCerts}
                    onChange={(event) => setInvalidCerts(event.target.checked)}
                  />
                  {t(language, "profileEditorInvalidCerts")}
                </label>
                <p className="muted">{t(language, "profileEditorTlsHelp")}</p>
                <p className="muted">{t(language, "profileEditorRouteHelp")}</p>
              </>
            )}
            <div className="form-actions">
              <button className="primary" disabled={busy}>
                {t(language, "profileEditorSave")}
              </button>
              <button type="button" disabled={busy} onClick={onClose}>
                {t(language, "profileEditorCancel")}
              </button>
            </div>
          </form>
        </div>
      </section>
    </div>
  );
}

export function ProfileCardItem({
  item,
  language,
  status,
  busy,
  rawEditing,
  deleting,
  perform,
  onEdit,
  onRawEdit,
  onMerge,
  onSequence,
  onScript,
  onDeleteStart,
  onDeleteCancel,
  onDeleteConfirm,
}: {
  item: Profile;
  language: Language;
  status: CoreStatus;
  busy: boolean;
  rawEditing?: string;
  deleting: string | null;
  perform: Perform;
  onEdit: () => void;
  onRawEdit: () => void;
  onMerge: () => void;
  onSequence: () => void;
  onScript: () => void;
  onDeleteStart: () => void;
  onDeleteCancel: () => void;
  onDeleteConfirm: () => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    function handleClickOutside(event: globalThis.MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setMenuOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [menuOpen]);

  return (
    <article
      className={`profile ${status.active_profile === item.uid ? "active-profile" : ""}`}
      key={item.uid}
    >
      <div className="profile-info">
        <div className="profile-heading-line">
          <h3>{item.name || item.uid}</h3>
          <span className="badge">
            {item.type === "remote"
              ? t(language, "profileRemote")
              : t(language, "profileLocal")}
          </span>
          {status.active_profile === item.uid && (
            <span className="badge good">{t(language, "profileCurrent")}</span>
          )}
        </div>
        <p className="mono muted">{item.uid}</p>
        {item.desc && (
          <p className="muted profile-description">{item.desc}</p>
        )}
        {item.extra && (
          <p className="muted">
            {t(language, "profileUsed")}{" "}
            {bytes(item.extra.upload + item.extra.download)} /{" "}
            {bytes(item.extra.total)}
          </p>
        )}
        <div className="profile-tags">
          {item.option?.merge && (
            <p className="muted">{t(language, "profileLinkedMerge")}</p>
          )}
          {item.option?.script && (
            <p className="muted">{t(language, "profileLinkedScript")}</p>
          )}
          {(item.option?.rules ||
            item.option?.proxies ||
            item.option?.groups) && (
            <p className="muted">{t(language, "profileLinkedSequence")}</p>
          )}
        </div>
      </div>
      <div className="profile-actions">
        {item.type === "remote" && (
          <button
            disabled={busy}
            aria-label={`${t(language, "profileRefresh")} ${item.name || item.uid}`}
            onClick={() =>
              void perform("refresh_profile", { uid: item.uid })
            }
          >
            {t(language, "profileRefresh")}
          </button>
        )}
        <button
          disabled={busy}
          className={status.active_profile === item.uid ? "primary" : ""}
          onClick={() =>
            void perform("select_profile", { uid: item.uid })
          }
        >
          {status.active_profile === item.uid
            ? t(language, "profileReapply")
            : t(language, "profileUse")}
        </button>
        <div className="profile-dropdown" ref={menuRef}>
          <button
            type="button"
            className="dropdown-trigger"
            aria-label={`${t(language, "profileMoreActions")} ${item.name || item.uid}`}
            title={t(language, "profileMoreActions")}
            onClick={() => setMenuOpen(!menuOpen)}
          >
            <svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">
              <circle cx="8" cy="3" r="1.4" />
              <circle cx="8" cy="8" r="1.4" />
              <circle cx="8" cy="13" r="1.4" />
            </svg>
          </button>
          {menuOpen && (
            <div className="dropdown-menu">
              <button
                type="button"
                disabled={busy}
                aria-label={`${t(language, "profileEditNamed")} ${item.name || item.uid}`}
                onClick={() => {
                  setMenuOpen(false);
                  onEdit();
                }}
              >
                {t(language, "profileEdit")}
              </button>
              <button
                type="button"
                disabled={busy || rawEditing !== undefined}
                aria-label={`${t(language, "profileEditRawNamed")} ${item.name || item.uid}`}
                onClick={() => {
                  setMenuOpen(false);
                  onRawEdit();
                }}
              >
                {t(language, "profileRawYaml")}
              </button>
              <button
                type="button"
                disabled={busy}
                aria-label={`${t(language, "profileMerge")} ${item.name || item.uid}`}
                onClick={() => {
                  setMenuOpen(false);
                  onMerge();
                }}
              >
                {t(language, "profileMerge")}
              </button>
              <button
                type="button"
                disabled={busy}
                aria-label={`${t(language, "profileSequence")} ${item.name || item.uid}`}
                onClick={() => {
                  setMenuOpen(false);
                  onSequence();
                }}
              >
                {t(language, "profileSequence")}
              </button>
              <button
                type="button"
                disabled={busy}
                aria-label={`${t(language, "profileScript")} ${item.name || item.uid}`}
                onClick={() => {
                  setMenuOpen(false);
                  onScript();
                }}
              >
                {t(language, "profileScript")}
              </button>
              <div className="dropdown-divider" />
              <button
                type="button"
                className="dropdown-item-danger"
                disabled={busy || status.active_profile === item.uid}
                aria-label={`${t(language, "profileDeleteNamed")} ${item.name || item.uid}`}
                title={
                  status.active_profile === item.uid
                    ? t(language, "profileSwitchFirst")
                    : undefined
                }
                onClick={() => {
                  setMenuOpen(false);
                  onDeleteStart();
                }}
              >
                {t(language, "profileDelete")}
              </button>
            </div>
          )}
        </div>
      </div>
      {deleting === item.uid && (
        <div
          className="modal-backdrop"
          onClick={(e) => {
            if (e.target === e.currentTarget) onDeleteCancel();
          }}
        >
          <div
            className="modal-dialog modal-dialog-sm delete-confirmation"
            role="dialog"
            aria-modal="true"
          >
            <div className="modal-header">
              <h3>{`${t(language, "profileDeleteNamed")} ${item.name || item.uid}`}</h3>
              <button
                type="button"
                className="modal-close"
                onClick={onDeleteCancel}
                aria-label={t(language, "close")}
              >
                ✕
              </button>
            </div>
            <div className="modal-body">
              <p>{t(language, "profileDeleteWarning")}</p>
              <div className="form-actions">
                <button
                  className="dropdown-item-danger"
                  disabled={busy}
                  onClick={onDeleteConfirm}
                  aria-label={`${t(language, "profileConfirmDelete")} ${item.name || item.uid}`}
                >
                  {t(language, "profileConfirmDelete")}
                </button>
                <button disabled={busy} onClick={onDeleteCancel}>
                  {t(language, "profileCancelDelete")}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </article>
  );
}

export function ProfilePage({
  token,
  language,
  logout,
  profiles,
  status,
  busy,
  perform,
}: {
  token: string;
  language: Language;
  logout: (reason?: string) => void;
  profiles: Profiles;
  status: CoreStatus;
  busy: boolean;
  perform: Perform;
}) {
  const [name, setName] = useState(""),
    [yaml, setYaml] = useState(""),
    [remoteUrl, setRemoteUrl] = useState(""),
    [remoteName, setRemoteName] = useState(""),
    [remoteSelfProxy, setRemoteSelfProxy] = useState(false),
    [remoteWithProxy, setRemoteWithProxy] = useState(false),
    [remoteInvalidCerts, setRemoteInvalidCerts] = useState(false),
    [showRemoteModal, setShowRemoteModal] = useState(false),
    [showLocalModal, setShowLocalModal] = useState(false),
    [error, setError] = useState<"too-large" | Error | null>(null);
  const [rawEditing, setRawEditing] = useState<string>();
  const [editing, setEditing] = useState<Profile | null>(null);
  const [mergeEditing, setMergeEditing] = useState<{
    item: Profile;
    content: { uid?: string; yaml?: string };
  } | null>(null);
  const [sequenceEditing, setSequenceEditing] = useState<{
    item: Profile;
    kind: SequenceKind;
    content: { uid?: string; yaml?: string };
  } | null>(null);
  const [scriptEditing, setScriptEditing] = useState<{
    item: Profile;
    content: { uid?: string; source?: string };
  } | null>(null);
  useEffect(() => {
    if (rawEditing && !profiles.items?.some((item) => item.uid === rawEditing))
      setRawEditing(undefined);
  }, [rawEditing, profiles]);
  const baseProfiles = profiles.items?.filter(
    (item) => item.type === "local" || item.type === "remote",
  );
  async function openMerge(item: Profile) {
    const content = await perform<{ uid?: string; yaml?: string }>(
      "profile_merge",
      { uid: item.uid },
    );
    if (content) setMergeEditing({ item, content });
  }
  async function openSequence(item: Profile, kind: SequenceKind = "rules") {
    const content = await perform<{ uid?: string; yaml?: string }>(
      "profile_sequence",
      { uid: item.uid, kind },
    );
    if (content) setSequenceEditing({ item, kind, content });
  }
  async function openScript(item: Profile) {
    const content = await perform<{ uid?: string; source?: string }>(
      "profile_script",
      { uid: item.uid },
    );
    if (content) setScriptEditing({ item, content });
  }
  const [deleting, setDeleting] = useState<string | null>(null);
  async function remove(item: Profile) {
    const result = await perform<Profiles>("delete_profile", { uid: item.uid });
    if (result) {
      setDeleting(null);
      if (editing?.uid === item.uid) setEditing(null);
      if (mergeEditing?.item.uid === item.uid) setMergeEditing(null);
      if (sequenceEditing?.item.uid === item.uid) setSequenceEditing(null);
      if (scriptEditing?.item.uid === item.uid) setScriptEditing(null);
    }
  }
  async function upload(file?: File) {
    if (!file) return;
    try {
      if (file.size > 8 * 1024 ** 2) {
        setError("too-large");
        return;
      }
      setYaml(await file.text());
      setName(file.name.replace(/\.ya?ml$/i, ""));
      setError(null);
    } catch (error) {
      setError(error instanceof Error ? error : new Error(String(error)));
    }
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    const item = await perform<Profile>("import_profile", { name, yaml });
    if (item) {
      setName("");
      setYaml("");
      setShowLocalModal(false);
    }
  }
  async function importRemote(event: FormEvent) {
    event.preventDefault();
    const item = await perform<Profile>("import_remote_profile", {
      url: remoteUrl.trim(),
      options: {
        self_proxy: remoteSelfProxy,
        with_proxy: remoteWithProxy,
        danger_accept_invalid_certs: remoteInvalidCerts,
      },
      ...(remoteName.trim() ? { name: remoteName.trim() } : {}),
    });
    if (item) {
      setRemoteUrl("");
      setRemoteName("");
      setShowRemoteModal(false);
    }
  }
  return (
    <div className="two-column profile-layout">
      <section className="panel profile-list">
        <div className="panel-title">
          <div>
            <h2>{t(language, "profileListTitle")}</h2>
            <span className="profile-count">
              {t(
                language,
                baseProfiles?.length === 1 ? "profileCountOne" : "profileCountOther",
              ).replace("{count}", String(baseProfiles?.length || 0))}
            </span>
          </div>
          <div className="profile-header-actions">
            <button
              type="button"
              className="primary"
              onClick={() => setShowRemoteModal(true)}
              aria-label={t(language, "profileAddRemote")}
            >
              {t(language, "profileAddRemote")}
            </button>
            <button
              type="button"
              onClick={() => setShowLocalModal(true)}
              aria-label={t(language, "profileAddLocal")}
            >
              {t(language, "profileAddLocal")}
            </button>
          </div>
        </div>
        <div className="profile-grid">
          {!baseProfiles?.length && (
            <div className="profile-empty">
              <p className="empty">{t(language, "profileEmpty")}</p>
            </div>
          )}
          {baseProfiles?.map((item) => (
            <ProfileCardItem
              key={item.uid}
              item={item}
              language={language}
              status={status}
              busy={busy}
              rawEditing={rawEditing}
              deleting={deleting}
              perform={perform}
              onEdit={() => setEditing(item)}
              onRawEdit={() => setRawEditing(item.uid)}
              onMerge={() => void openMerge(item)}
              onSequence={() => void openSequence(item)}
              onScript={() => void openScript(item)}
              onDeleteStart={() => setDeleting(item.uid)}
              onDeleteCancel={() => setDeleting(null)}
              onDeleteConfirm={() => void remove(item)}
            />
          ))}
        </div>
      </section>
      <GlobalEnhancements language={language} status={status} busy={busy} perform={perform} />
      {rawEditing &&
        profiles.items?.some((item) => item.uid === rawEditing) && (
          <RawEditor
            key={rawEditing}
            item={profiles.items.find((item) => item.uid === rawEditing)!}
            active={status.active_profile === rawEditing}
            language={language}
            token={token}
            busy={busy}
            perform={perform}
            logout={logout}
            onClose={() => setRawEditing(undefined)}
          />
        )}
      {mergeEditing &&
        profiles.items?.some((item) => item.uid === mergeEditing.item.uid) && (
          <MergeEditor
            key={mergeEditing.item.uid}
            item={mergeEditing.item}
            language={language}
            content={mergeEditing.content}
            busy={busy}
            perform={perform}
            onClose={() => setMergeEditing(null)}
          />
        )}
      {sequenceEditing &&
        profiles.items?.some(
          (item) => item.uid === sequenceEditing.item.uid,
        ) && (
          <SequenceEditor
            key={`${sequenceEditing.item.uid}-${sequenceEditing.kind}`}
            item={sequenceEditing.item}
            language={language}
            kind={sequenceEditing.kind}
            content={sequenceEditing.content}
            busy={busy}
            perform={perform}
            onKind={(kind) => void openSequence(sequenceEditing.item, kind)}
            onClose={() => setSequenceEditing(null)}
          />
        )}
      {scriptEditing &&
        profiles.items?.some((item) => item.uid === scriptEditing.item.uid) && (
          <ScriptEditor
            key={scriptEditing.item.uid}
            item={scriptEditing.item}
            language={language}
            content={scriptEditing.content}
            busy={busy}
            perform={perform}
            onClose={() => setScriptEditing(null)}
          />
        )}
      {editing && profiles.items?.some((item) => item.uid === editing.uid) && (
        <ProfileEditor
          key={editing.uid}
          item={editing}
          language={language}
          busy={busy}
          perform={perform}
          onClose={() => setEditing(null)}
        />
      )}
      {showRemoteModal && (
        <div
          className="modal-backdrop"
          onClick={(e) => {
            if (e.target === e.currentTarget) setShowRemoteModal(false);
          }}
        >
          <section className="modal-dialog" aria-label={t(language, "remoteImportTitle")}>
            <div className="modal-header">
              <h2>{t(language, "remoteImportTitle")}</h2>
              <button
                type="button"
                className="modal-close"
                onClick={() => setShowRemoteModal(false)}
                aria-label={t(language, "close")}
              >
                ✕
              </button>
            </div>
            <div className="modal-body">
              <p className="muted">{t(language, "remoteImportHelp")}</p>
              <form onSubmit={importRemote}>
                <label>
                  {t(language, "remoteImportUrl")}
                  <input
                    type="url"
                    value={remoteUrl}
                    required
                    maxLength={8192}
                    disabled={busy}
                    placeholder="https://example.com/subscription"
                    onChange={(event) => setRemoteUrl(event.target.value)}
                  />
                </label>
                <label>
                  {t(language, "remoteImportName")}
                  <input
                    value={remoteName}
                    maxLength={256}
                    disabled={busy}
                    onChange={(event) => setRemoteName(event.target.value)}
                  />
                </label>
                <label className="check-label">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={remoteSelfProxy}
                    onChange={(event) => setRemoteSelfProxy(event.target.checked)}
                  />
                  {t(language, "remoteImportManaged")}
                </label>
                <label className="check-label">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={remoteWithProxy}
                    onChange={(event) => setRemoteWithProxy(event.target.checked)}
                  />
                  {t(language, "remoteImportSystem")}
                </label>
                <label className="check-label">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={remoteInvalidCerts}
                    onChange={(event) => setRemoteInvalidCerts(event.target.checked)}
                  />
                  {t(language, "remoteImportInvalidCerts")}
                </label>
                <p className="muted">{t(language, "remoteImportTlsHelp")}</p>
                <p className="muted">{t(language, "remoteImportRouteHelp")}</p>
                <div className="form-actions">
                  <button className="primary" disabled={busy || !remoteUrl.trim()}>
                    {t(language, "remoteImportSubmit")}
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => setShowRemoteModal(false)}
                  >
                    {t(language, "cancel")}
                  </button>
                </div>
              </form>
            </div>
          </section>
        </div>
      )}
      {showLocalModal && (
        <div
          className="modal-backdrop"
          onClick={(e) => {
            if (e.target === e.currentTarget) setShowLocalModal(false);
          }}
        >
          <section className="modal-dialog" aria-label={t(language, "localImportTitle")}>
            <div className="modal-header">
              <h2>{t(language, "localImportTitle")}</h2>
              <button
                type="button"
                className="modal-close"
                onClick={() => setShowLocalModal(false)}
                aria-label={t(language, "close")}
              >
                ✕
              </button>
            </div>
            <div className="modal-body">
              <p className="muted">{t(language, "localImportHelp")}</p>
              <form onSubmit={submit}>
                <label>
                  {t(language, "localImportUpload")}
                  <input
                    type="file"
                    accept=".yaml,.yml,text/yaml,text/plain"
                    disabled={busy}
                    onChange={(event) => void upload(event.target.files?.[0])}
                  />
                </label>
                <label>
                  {t(language, "localImportName")}
                  <input
                    value={name}
                    required
                    maxLength={256}
                    disabled={busy}
                    onChange={(event) => setName(event.target.value)}
                  />
                </label>
                <label>
                  {t(language, "localImportYaml")}
                  <textarea
                    className="code small"
                    value={yaml}
                    required
                    spellCheck={false}
                    disabled={busy}
                    onChange={(event) => setYaml(event.target.value)}
                  />
                </label>
                {error && (
                  <p className="alert" role="alert">
                    {error === "too-large" ? t(language, "localImportFileTooLarge") : describe(error)}
                  </p>
                )}
                <div className="form-actions">
                  <button
                    className="primary"
                    disabled={busy || !yaml.trim() || !name.trim()}
                  >
                    {t(language, "localImportSubmit")}
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => setShowLocalModal(false)}
                  >
                    {t(language, "cancel")}
                  </button>
                </div>
              </form>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}
