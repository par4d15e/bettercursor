import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSyncStore } from "../store/syncStore";
import { transportPull, transportPush } from "../lib/tauri";
import { useSessionStore } from "../store/sessionStore";
import { X, Radio, Users } from "lucide-react";

interface Props {
  open: boolean;
  onClose: () => void;
}

export function SyncPeersDialog({ open, onClose }: Props) {
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
    if (!open) return;
    void refresh();
    void browse();
  }, [open, refresh, browse]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-bg-secondary border border-border rounded-lg w-full max-w-lg max-h-[80vh] overflow-auto p-4 shadow-xl">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-semibold flex items-center gap-2">
            <Users size={18} />
            {t("sync.peers.title")}
          </h2>
          <button type="button" onClick={onClose} aria-label={t("common.close")}>
            <X size={18} />
          </button>
        </div>

        {error && (
          <p className="text-sm text-red-400 mb-2">{error}</p>
        )}
        {status && (
          <p className="text-sm text-green-400 mb-2">{status}</p>
        )}

        <section className="mb-4">
          <h3 className="text-sm font-medium mb-1">{t("sync.peers.pairing")}</h3>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="px-3 py-1 text-sm rounded bg-accent text-white disabled:opacity-50"
              disabled={loading}
              onClick={() => void startPairing()}
            >
              {t("sync.peers.showCode")}
            </button>
            {pairingCode && (
              <span className="text-sm font-mono">
                {t("sync.peers.code", { code: pairingCode, port: pairingPort ?? 0 })}
              </span>
            )}
          </div>
        </section>

        <section className="mb-4">
          <h3 className="text-sm font-medium mb-1 flex items-center gap-1">
            <Radio size={14} />
            {t("sync.peers.nearby")}
          </h3>
          <button
            type="button"
            className="text-xs underline mb-2"
            onClick={() => void browse()}
          >
            {t("sync.peers.rescan")}
          </button>
          <ul className="space-y-1 text-sm">
            {discovered.map((d) => (
              <li key={`${d.device_id}-${d.port}`} className="flex items-center gap-2">
                <span>{d.device_name} ({d.host}:{d.port})</span>
                <input
                  className="border border-border rounded px-1 w-20 text-xs"
                  placeholder={t("sync.peers.codePlaceholder")}
                  value={joinCode}
                  onChange={(e) => setJoinCode(e.target.value)}
                />
                <button
                  type="button"
                  className="text-xs px-2 py-0.5 rounded bg-bg-primary border border-border"
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
              <li className="text-fg-muted">{t("sync.peers.noNearby")}</li>
            )}
          </ul>
        </section>

        <section className="mb-4">
          <h3 className="text-sm font-medium mb-1">{t("sync.peers.trusted")}</h3>
          <ul className="space-y-1 text-sm">
            {trustedPeers.map((p) => (
              <li key={p.id} className="flex items-center justify-between gap-2">
                <span>{p.device_name} — {p.lan_addr}</span>
                <div className="flex gap-1">
                  <button
                    type="button"
                    className="text-xs px-2 py-0.5 rounded border border-border"
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
                    className="text-xs px-2 py-0.5 rounded border border-border"
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
              <li className="text-fg-muted">{t("sync.peers.noTrusted")}</li>
            )}
          </ul>
        </section>
      </div>
    </div>
  );
}
