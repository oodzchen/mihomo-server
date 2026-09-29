/** Display text for a thrown value. */
export const describe = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

/** Human-readable size for byte counts reported by the core. */
export const bytes = (value?: number) =>
  value === undefined
    ? "—"
    : value >= 1024 ** 2
      ? `${(value / 1024 ** 2).toFixed(1)} MB`
      : `${(value / 1024).toFixed(1)} KB`;
