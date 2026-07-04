// src/components/SyncNowButton.tsx — manual "立即同步" trigger.
//
// v0.2.3: extracted from SessionTree's inline button. Wraps the
// `syncNow` store action with a per-call loading spinner so the user
// gets visual feedback while the backend re-scans Cursor's three
// storage layers. The actual state refresh happens via the
// `sessions-updated` Tauri event (subscribed in `init`), not via
// direct listSessions — this button just kicks off the scan.

import { useState } from "react";
import { RefreshCw } from "lucide-react";
import { useSessionStore } from "../store/sessionStore";

export function SyncNowButton() {
  const syncNow = useSessionStore((s) => s.syncNow);
  const loading = useSessionStore((s) => s.loading);
  const [localLoading, setLocalLoading] = useState(false);

  // Combine store-level loading (set by syncNow action) with local
  // click-debounce. If the action is already running, don't double-fire.
  const busy = loading || localLoading;

  return (
    <button
      type="button"
      data-testid="sync-now"
      className="p-1 rounded hover:bg-bg-hover disabled:opacity-50"
      title="立即重新扫描本机 Cursor 存储"
      disabled={busy}
      onClick={async () => {
        if (busy) return;
        setLocalLoading(true);
        try {
          await syncNow();
        } finally {
          setLocalLoading(false);
        }
      }}
    >
      <RefreshCw size={14} className={busy ? "animate-spin" : ""} />
    </button>
  );
}