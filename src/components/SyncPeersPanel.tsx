import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSyncStore } from "../store/syncStore";
import {
  getPreferences,
  setPreferences,
  transportPull,
  transportPush,
  type SessionPullResult,
} from "../lib/tauri";
import { useSessionStore } from "../store/sessionStore";
import { Radio } from "lucide-react";

interface Props {
  active: boolean;
}

function formatPullResults(
  results: SessionPullResult[],
  t: (key: string, opts?: Record<string, unknown>) => string,
): string[] {
  return results.map((r) => {
    const l2 = r.applied_l2 ? "✓" : "—";
    const l3 = r.applied_l3 ? "✓" : "—";
    let skip = "";
    if (r.skipped.length > 0) {
      skip = t("sync.peers.pullDetailSkip", { reason: r.skipped.join(", ") });
    }
    if (r.error) {
      skip += t("sync.peers.pullDetailError", { error: r.error });
    }
    return t("sync.peers.pullDetail", {
      uuid: r.uuid.slice(0, 8),
      class: r.conflict_class,
      l2,
      l3,
      skip,
    });
  });
}

export function SyncPeersPanel({ active }: Props) {
  const { t } = useTranslation();
  const {
    trustedPeers,
    discovered,
    pairingCode,
    pairingPort,
    loading,
    error,
    refresh,
    browse,
    startPairing,
    joinPeer,
  } = useSyncStore();
  const selectedUuid = useSessionStore((s) => s.selectedUuid);
  const [joinCode, setJoinCode] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [pullDetails, setPullDetails] = useState<string[]>([]);
  const [autoPullEnabled, setAutoPullEnabled] = useState(true);
  const [autoPullInterval, setAutoPullInterval] = useState(300);
  const [pathMappings, setPathMappings] = useState<Record<string, string>>({});

  useEffect(() => {
    if (!active) return;
    void refresh();
    void getPreferences().then((p) => {
      setAutoPullEnabled(p.auto_pull_enabled);
      setAutoPullInterval(p.auto_pull_interval_secs);
      setPathMappings(p.path_mappings ?? {});
    });
  }, [active, refresh]);

  const savePrefs = (enabled: boolean, interval: number) => {
    void setPreferences({
      path_mappings: pathMappings,
      auto_pull_enabled: enabled,
      auto_pull_interval_secs: interval,
    });
  };

  return (
    <div className="space-y-4 text-sm">
      {error && <p className="text-sm text-red-400">{error}</p>}
      {status && <p className="text-sm text-accent-green">{status}</p>}
      {pullDetails.length > 0 && (
        <ul className="text-xs text-fg-muted space-y-0.5 font-mono">
          {pullDetails.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      )}

      <div className="flex flex-wrap items-center gap-3 border border-border rounded p-2">
        <label className="flex items-center gap-2 text-fg-secondary">
          <input
            type="checkbox"
            checked={autoPullEnabled}
            onChange={(e) => {
              setAutoPullEnabled(e.target.checked);
              savePrefs(e.target.checked, autoPullInterval);
            }}
          />
          {t("sync.peers.autoPull")}
        </label>
        <label className="flex items-center gap-2 text-fg-secondary text-xs">
          {t("sync.peers.autoPullInterval")}
          <input
            type="number"
            min={60}
            max={3600}
            className="bg-bg-tertiary border border-border rounded px-1.5 py-0.5 w-20 text-xs"
            value={autoPullInterval}
            onChange={(e) => {
              const n = Math.max(60, Number(e.target.value) || 300);
              setAutoPullInterval(n);
              savePrefs(autoPullEnabled, n);
            }}
          />
        </label>
      </div>

      <div>
        <h4 className="text-xs font-medium text-fg-secondary mb-1">
          {t("sync.peers.pairing")}
        </h4>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            className="px-3 py-1 text-sm rounded bg-accent-blue text-white disabled:opacity-50"
            disabled={loading}
            onClick={() => void startPairing()}
          >
            {t("sync.peers.showCode")}
          </button>
          {pairingCode && (
            <span className="text-sm font-mono text-fg-secondary">
              {t("sync.peers.code", { code: pairingCode, port: pairingPort ?? 0 })}
            </span>
          )}
        </div>
      </div>

      <div>
        <h4 className="text-xs font-medium text-fg-secondary mb-1 flex items-center gap-1">
          <Radio size={14} />
          {t("sync.peers.nearby")}
        </h4>
        <button
          type="button"
          className="text-xs text-fg-muted hover:text-fg-primary underline mb-2"
          onClick={() => void browse()}
        >
          {t("sync.peers.rescan")}
        </button>
        <ul className="space-y-2">
          {discovered.map((d) => (
            <li
              key={`${d.device_id}-${d.port}`}
              className="flex flex-wrap items-center gap-2 border border-border rounded p-2"
            >
              <span className="text-fg-primary">
                {d.device_name} ({d.host}:{d.port})
              </span>
              <input
                className="bg-bg-tertiary border border-border rounded px-1.5 py-0.5 w-20 text-xs text-fg-primary"
                placeholder={t("sync.peers.codePlaceholder")}
                value={joinCode}
                onChange={(e) => setJoinCode(e.target.value)}
              />
              <button
                type="button"
                className="text-xs px-2 py-0.5 rounded border border-border hover:bg-bg-hover"
                onClick={() =>
                  void joinPeer(d.host, d.port, joinCode).then(() =>
                    setStatus(t("sync.peers.paired")),
                  )
                }
              >
                {t("sync.peers.join")}
              </button>
            </li>
          ))}
          {discovered.length === 0 && (
            <li className="text-fg-muted text-xs">{t("sync.peers.noNearby")}</li>
          )}
        </ul>
      </div>

      <div>
        <h4 className="text-xs font-medium text-fg-secondary mb-1">
          {t("sync.peers.trusted")}
        </h4>
        <ul className="space-y-2">
          {trustedPeers.map((p) => (
            <li
              key={p.id}
              className="flex flex-wrap items-center justify-between gap-2 border border-border rounded p-2"
            >
              <span className="text-fg-primary">
                {p.device_name} — {p.lan_addr}
              </span>
              <div className="flex gap-1">
                <button
                  type="button"
                  className="text-xs px-2 py-0.5 rounded border border-border hover:bg-bg-hover disabled:opacity-50"
                  disabled={!selectedUuid}
                  onClick={() => {
                    if (!selectedUuid) return;
                    void transportPush(selectedUuid, p.id).then(
                      () => setStatus(t("sync.peers.pushed")),
                      (e) => setStatus(String(e)),
                    );
                  }}
                >
                  {t("sync.peers.push")}
                </button>
                <button
                  type="button"
                  className="text-xs px-2 py-0.5 rounded border border-border hover:bg-bg-hover"
                  onClick={() =>
                    void transportPull(p.id).then(
                      (r) => {
                        setStatus(
                          t("sync.peers.pulled", {
                            count: r.count,
                            applied: r.applied,
                            skipped: r.skipped_count,
                            failed: r.failed,
                          }),
                        );
                        setPullDetails(formatPullResults(r.results ?? [], t));
                      },
                      (e) => setStatus(String(e)),
                    )
                  }
                >
                  {t("sync.peers.pull")}
                </button>
              </div>
            </li>
          ))}
          {trustedPeers.length === 0 && (
            <li className="text-fg-muted text-xs">{t("sync.peers.noTrusted")}</li>
          )}
        </ul>
      </div>
    </div>
  );
}
