import { create } from "zustand";
import {
  discoveryBrowse,
  listTrustedPeers,
  listUnresolvedConflicts,
  pairingJoin,
  pairingStart,
  type ConflictRow,
  type DiscoveredDevice,
  type TrustedPeer,
} from "../lib/tauri";

interface SyncStore {
  trustedPeers: TrustedPeer[];
  discovered: DiscoveredDevice[];
  conflicts: ConflictRow[];
  pairingCode: string | null;
  pairingPort: number | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  browse: () => Promise<void>;
  startPairing: () => Promise<void>;
  joinPeer: (host: string, port: number, code: string) => Promise<void>;
  loadConflicts: () => Promise<void>;
}

export const useSyncStore = create<SyncStore>((set, get) => ({
  trustedPeers: [],
  discovered: [],
  conflicts: [],
  pairingCode: null,
  pairingPort: null,
  loading: false,
  error: null,

  refresh: async () => {
    set({ loading: true, error: null });
    try {
      const trustedPeers = await listTrustedPeers();
      set({ trustedPeers, loading: false });
    } catch (e) {
      set({
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  browse: async () => {
    set({ loading: true, error: null });
    try {
      const discovered = await discoveryBrowse();
      set({ discovered, loading: false });
    } catch (e) {
      set({
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  startPairing: async () => {
    set({ loading: true, error: null });
    try {
      const r = await pairingStart();
      set({
        pairingCode: r.code,
        pairingPort: r.port,
        loading: false,
      });
    } catch (e) {
      set({
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  joinPeer: async (host, port, code) => {
    set({ loading: true, error: null });
    try {
      const deviceName =
        typeof window !== "undefined" ? window.location.hostname : "bettercursor";
      await pairingJoin(host, port, code, deviceName);
      await get().refresh();
      set({ loading: false });
    } catch (e) {
      set({
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  loadConflicts: async () => {
    try {
      const conflicts = await listUnresolvedConflicts();
      set({ conflicts });
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
    }
  },
}));
