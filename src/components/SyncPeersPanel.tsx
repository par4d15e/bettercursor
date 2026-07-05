import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSyncStore } from "../store/syncStore";
import { transportPull, transportPush } from "../lib/tauri";
import { useSessionStore } from "../store/sessionStore";
import { Radio } from "lucide-react";

interface Props {
  active: boolean;
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

  useEffect(() => {
    if (!active) return;
    void refresh();
    void browse();
  }, [active, refresh, browse]);

  return (
    <div className="space-y-4 text-sm">
      {error && <p className="text-sm text-red-400">{error}</p>}
      {status && <p className="text-sm text-accent-green">{status}</p>}

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
                      (r) => setStatus(t("sync.peers.pulled", { count: r.count })),
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
