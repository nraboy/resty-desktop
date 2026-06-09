import { create } from "zustand";
import type { Repository, Snapshot } from "../lib/types";

interface AppState {
  activeRepo: Repository | null;
  setActiveRepo: (repo: Repository | null) => void;
  activeSnapshot: Snapshot | null;
  setActiveSnapshot: (snapshot: Snapshot | null) => void;
}

export const useAppStore = create<AppState>((set) => ({
  activeRepo: null,
  setActiveRepo: (repo) => set({ activeRepo: repo, activeSnapshot: null }),
  activeSnapshot: null,
  setActiveSnapshot: (snapshot) => set({ activeSnapshot: snapshot }),
}));
