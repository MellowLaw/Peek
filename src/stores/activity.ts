import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ActivityHubState } from "../types/peek";

const EMPTY_HUB: ActivityHubState = {
  activeBanner: null,
  ongoing: [],
  notifications: [],
  unreadCount: 0,
};

type ActivityStore = ActivityHubState & {
  init: () => () => void;
  refresh: () => Promise<void>;
  dismissBanner: (id: string) => Promise<void>;
  dismissNotification: (id: string) => Promise<void>;
  clearNotifications: () => Promise<void>;
};

export const useActivity = create<ActivityStore>((set) => ({
  ...EMPTY_HUB,
  init: () => {
    let active = true;
    void invoke<ActivityHubState>("get_activity_hub")
      .then((hub) => {
        if (active) set(hub);
      })
      .catch(() => {});
    const sub = listen<ActivityHubState>("activity-state", ({ payload }) => {
      if (active) set(payload);
    });
    return () => {
      active = false;
      void sub.then((unlisten) => unlisten()).catch(() => {});
    };
  },
  refresh: async () => {
    try {
      const hub = await invoke<ActivityHubState>("get_activity_hub");
      set(hub);
    } catch {
      // ignore
    }
  },
  dismissBanner: async (id) => {
    await invoke("dismiss_activity", { id }).catch(() => {});
  },
  dismissNotification: async (id) => {
    await invoke("dismiss_notification", { id }).catch(() => {});
  },
  clearNotifications: async () => {
    await invoke("clear_notifications").catch(() => {});
  },
}));
