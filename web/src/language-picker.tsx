import { t, type Language } from "./i18n";

export function LanguagePicker({
  language,
  changeLanguage,
}: {
  language: Language;
  changeLanguage: (value: string) => void;
}) {
  return (
    <label className="language-picker">
      {t(language, "language")}
      <select
        aria-label={t(language, "language")}
        value={language}
        onChange={(event) => changeLanguage(event.target.value)}
      >
        <option value="zh">简体中文</option>
        <option value="zhtw">繁體中文</option>
        <option value="en">English</option>
      </select>
    </label>
  );
}
