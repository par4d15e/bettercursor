import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useSyncStore } from "../store/syncStore";
import { resolveConflict } from "../lib/tauri";
import { CheckCircle2 } from "lucide-react";

interface Props {
  active: boolean;
}

export function ConflictResolvePanel({ active }: Props) {
  const { t } = useTranslation();
  const { conflicts, loadConflicts, error } = useSyncStore();

  useEffect(() => {
    if (active) void loadConflicts();
  }, [active, loadConflicts]);

  const hasConflicts = conflicts.length > 0;

  return (
    <div className="text-sm">
      <p className="text-xs text-fg-muted mb-3">{t("sync.conflicts.description")}</p>

      {error && <p className="text-sm text-red-400 mb-2">{error}</p>}

      {hasConflicts ? (
        <ul className="space-y-2 max-h-48 overflow-auto">
          {conflicts.map((c) => (
            <li
              key={c.id}
              className="border border-border rounded p-2 flex flex-col gap-1"
            >
              <span className="font-mono text-xs text-fg-secondary">{c.session_uuid}</span>
              <span className="text-fg-primary">{c.class}</span>
              <div className="flex gap-2">
                <button
                  type="button"
                  className="text-xs px-2 py-0.5 rounded border border-border hover:bg-bg-hover"
                  onClick={() =>
                    void resolveConflict(c.id, "auto_merged").then(() => loadConflicts())
                  }
                >
                  {t("sync.conflicts.acceptMerged")}
                </button>
                <button
                  type="button"
                  className="text-xs px-2 py-0.5 rounded border border-border hover:bg-bg-hover"
                  onClick={() =>
                    void resolveConflict(c.id, "skipped").then(() => loadConflicts())
                  }
                >
                  {t("sync.conflicts.skip")}
                </button>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <div className="py-4 text-center text-fg-muted">
          <CheckCircle2 size={24} className="mx-auto mb-2 text-accent-green" />
          <p className="text-fg-secondary text-sm">{t("sync.conflicts.none")}</p>
          <p className="text-xs mt-1">{t("sync.conflicts.noneHint")}</p>
        </div>
      )}
    </div>
  );
}
