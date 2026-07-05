import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Settings, X } from "lucide-react";
import { LanguageSwitcher } from "./LanguageSwitcher";
import { SyncPeersPanel } from "./SyncPeersPanel";
import { ConflictResolvePanel } from "./ConflictResolvePanel";

interface Props {
  open: boolean;
  onClose: () => void;
}

function SettingsSection({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section className="border-b border-border pb-4 last:border-b-0 last:pb-0">
      <h3 className="text-sm font-medium text-fg-primary mb-3">{title}</h3>
      {children}
    </section>
  );
}

export function SettingsDialog({ open, onClose }: Props) {
  const { t } = useTranslation();

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-bg-secondary border border-border rounded-lg w-full max-w-lg max-h-[85vh] flex flex-col shadow-xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-border shrink-0">
          <h2 className="text-lg font-semibold flex items-center gap-2">
            <Settings size={18} className="text-fg-secondary" />
            {t("settings.title")}
          </h2>
          <button type="button" onClick={onClose} aria-label={t("common.close")}>
            <X size={18} />
          </button>
        </div>

        <div className="overflow-auto px-4 py-4 space-y-4">
          <SettingsSection title={t("settings.language")}>
            <LanguageSwitcher size="md" />
          </SettingsSection>

          <SettingsSection title={t("settings.crossDeviceSync")}>
            <SyncPeersPanel active={open} />
          </SettingsSection>

          <SettingsSection title={t("settings.conflicts")}>
            <ConflictResolvePanel active={open} />
          </SettingsSection>
        </div>
      </div>
    </div>
  );
}
