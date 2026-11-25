import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type UpdateInfo = {
  version: string;
  currentVersion: string;
  notes: string;
  fullNotes: string;
  pubDate: string | null;
};

export type UpdateStateSnapshot = {
  status: "idle" | "checking" | "available" | "downloading" | "installing" | "upToDate" | "error";
  currentVersion: string;
  available: UpdateInfo | null;
  progressPercent: number | null;
  downloadedBytes: number;
  totalBytes: number | null;
  error: string | null;
  dismissed: boolean;
};

const EMPTY_UPDATE: UpdateStateSnapshot = {
  status: "idle",
  currentVersion: "0.3.4",
  available: null,
  progressPercent: null,
  downloadedBytes: 0,
  totalBytes: null,
  error: null,
  dismissed: false,
};

type UpdaterStore = {
  snapshot: UpdateStateSnapshot;
  init: () => () => void;
  check: (manual?: boolean) => Promise<void>;
  install: () => Promise<void>;
  dismiss: () => Promise<void>;
  skip: () => Promise<void>;
  openPopup: () => Promise<void>;
};

export const useUpdater = create<UpdaterStore>((set) => ({
  snapshot: EMPTY_UPDATE,
  init: () => {
    let active = true;
    void invoke<UpdateStateSnapshot>("get_update_state")
      .then((snap) => {
        if (active) set({ snapshot: snap });
      })
      .catch(() => {});
    const sub = listen<UpdateStateSnapshot>("app-update-state", ({ payload }) => {
      if (active) set({ snapshot: payload });
    });
    return () => {
      active = false;
      void sub.then((unlisten) => unlisten()).catch(() => {});
    };
  },
  check: async (manual = true) => {
    try {
      const snap = await invoke<UpdateStateSnapshot>("check_for_updates", { manual });
      set({ snapshot: snap });
    } catch {
      // Errors are reflected via snapshot state
    }
  },
  install: async () => {
    await invoke("install_available_update").catch(() => {});
  },
  dismiss: async () => {
    const snap = await invoke<UpdateStateSnapshot>("dismiss_update_prompt").catch(() => null);
    if (snap) set({ snapshot: snap });
  },
  skip: async () => {
    const snap = await invoke<UpdateStateSnapshot>("skip_available_update").catch(() => null);
    if (snap) set({ snapshot: snap });
  },
  openPopup: async () => {
    await invoke("open_floating_update").catch(() => {});
  },
}));
