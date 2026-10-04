import { useEffect, useState } from "react";
import { t, type Language } from "./i18n";

export function LanguagePicker({
  language,
  changeLanguage,
  autoApply = false,
}: {
  language: Language;
  autoApply?: boolean;
  changeLanguage: (value: string) => void;
}) {
  const [draft, setDraft] = useState<string>(language);
  useEffect(() => setDraft(language), [language]);
  const apply = () => { if (draft !== language) changeLanguage(draft); };
  return (
    <label className="language-picker">
      {t(language, "language")}
      <select
        aria-label={t(language, "language")}
        value={autoApply ? draft : language}
        onChange={event => autoApply ? setDraft(event.target.value) : changeLanguage(event.target.value)}
        onBlur={autoApply ? apply : undefined}
        onKeyDown={autoApply ? event => {
          if (event.key === "Enter" && !event.nativeEvent.isComposing) { event.preventDefault(); apply(); }
        } : undefined}
      >
        <option value="zh">简体中文</option>
        <option value="zhtw">繁體中文</option>
        <option value="en">English</option>
      </select>
    </label>
  );
}
