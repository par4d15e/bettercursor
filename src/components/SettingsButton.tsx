import { useTranslation } from "react-i18next";
import { Settings } from "lucide-react";
import { useSyncStore } from "../store/syncStore";

interface Props {
  onClick: () => void;
}

export function SettingsButton({ onClick }: Props) {
  const { t } = useTranslation();
  const conflicts = useSyncStore((s) => s.conflicts);

  const count = conflicts.length;
  const title =
    count > 0
      ? t("settings.buttonTitleWithConflicts", { count })
      : t("settings.buttonTitle");

  return (
    <button
      type="button"
      data-testid="settings-button"
      className="relative p-1 rounded hover:bg-bg-hover text-fg-secondary"
      title={title}
      aria-label={title}
      onClick={onClick}
    >
      <Settings size={14} />
      {count > 0 && (
        <span className="absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] px-0.5 rounded-full bg-accent-yellow text-[9px] font-mono leading-[14px] text-center text-bg-primary">
          {count > 9 ? "9+" : count}
        </span>
      )}
    </button>
  );
}
