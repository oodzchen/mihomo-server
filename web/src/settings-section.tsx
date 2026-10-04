import type { ReactNode } from "react";

/** Advanced settings stay mounted so collapsed drafts are included in automatic application. */
export function SettingsSection({ title, children }: { title: string; children: ReactNode }) {
  return <details className="settings-group"><summary>{title}</summary>{children}</details>;
}
