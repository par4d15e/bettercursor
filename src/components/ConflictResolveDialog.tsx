import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useSyncStore } from "../store/syncStore";
import { resolveConflict } from "../lib/tauri";
import { X, AlertTriangle } from "lucide-react";

interface Props {
  open: boolean;
  onClose: () => void;
}

export function ConflictResolveDialog({ open, onClose }: Props) {
  const { t } = useTranslation();
  const { conflicts, loadConflicts, error } = useSyncStore();

  useEffect(() => {
    if (open) void loadConflicts();
  }, [open, loadConflicts]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-bg-secondary border border-border rounded-lg w-full max-w-md p-4 shadow-xl">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-semibold flex items-center gap-2">
            <AlertTriangle size={18} />
            {t("sync.conflicts.title")}
          </h2>
          <button type="button" onClick={onClose} aria-label={t("common.close")}>
            <X size={18} />
          </button>
        </div>

        {error && <p className="text-sm text-red-400 mb-2">{error}</p>}

        <ul className="space-y-2 text-sm max-h-64 overflow-auto">
          {conflicts.map((c) => (
            <li
              key={c.id}
              className="border border-border rounded p-2 flex flex-col gap-1"
            >
              <span className="font-mono text-xs">{c.session_uuid}</span>
              <span>{c.class}</span>
              <div className="flex gap-2">
                <button
                  type="button"
                  className="text-xs px-2 py-0.5 rounded border border-border"
                  onClick={() =>
                    void resolveConflict(c.id, "auto_merged").then(() =>
                      loadConflicts(),
                    )
                  }
                >
                  {t("sync.conflicts.acceptMerged")}
                </button>
                <button
                  type="button"
                  className="text-xs px-2 py-0.5 rounded border border-border"
                  onClick={() =>
                    void resolveConflict(c.id, "skipped").then(() =>
                      loadConflicts(),
                    )
                  }
                >
                  {t("sync.conflicts.skip")}
                </button>
              </div>
            </li>
          ))}
          {conflicts.length === 0 && (
            <li className="text-fg-muted">{t("sync.conflicts.none")}</li>
          )}
        </ul>
      </div>
    </div>
  );
}
