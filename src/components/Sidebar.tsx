import { useEffect, useState } from "react";
import { NavLink } from "react-router-dom";
import { getVersion } from "@tauri-apps/api/app";
import { LockIcon } from "./icons";
import { useActivity, activeTaskCount } from "../lib/activity";

const navItems = [
  {
    to: "/",
    label: "Repositories",
    icon: (
      <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
          d="M3 7a2 2 0 012-2h14a2 2 0 012 2v10a2 2 0 01-2 2H5a2 2 0 01-2-2V7z" />
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
          d="M8 7v10M16 7v10" />
      </svg>
    ),
  },
  {
    to: "/backup-plans",
    label: "Backup Plans",
    icon: (
      <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
          d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4" />
      </svg>
    ),
  },
  {
    to: "/schedules",
    label: "Schedules",
    icon: (
      <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
          d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
      </svg>
    ),
  },
  {
    to: "/logs",
    label: "Logs",
    icon: (
      <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
          d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
      </svg>
    ),
  },
  {
    to: "/settings",
    label: "Settings",
    icon: (
      <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
          d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
          d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
      </svg>
    ),
  },
];

interface SidebarProps {
  /** Locks the session. Only passed when the app is unlocked (Sidebar renders solely in
   *  App.tsx's unlocked branch), so its presence doubles as that gate. */
  onLock?: () => void;
  /** Whether the Activity drawer is currently open — drives the Activity item's highlight. */
  activityOpen: boolean;
  /** Toggles the Activity drawer (the same drawer the right-edge rail opens — state lives in
   *  App.tsx; see ActivityPanel). */
  onToggleActivity: () => void;
}

export default function Sidebar({ onLock, activityOpen, onToggleActivity }: SidebarProps) {
  const [appVersion, setAppVersion] = useState("");
  const activity = useActivity();
  // Same shared counter the panel's empty-state and rail dot derive from (see activity.tsx), so
  // the badge can never disagree with what the drawer shows. Includes queued batches/mirrors.
  const activityCount = activeTaskCount(activity);

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  // The route nav items' NavLink body — extracted so the two slices around the Activity entry
  // (below) share one rendering path instead of duplicating it.
  const renderNavLink = (item: (typeof navItems)[number]) => (
    <NavLink
      key={item.to}
      to={item.to}
      end={item.to === "/"}
      className={({ isActive }) =>
        `flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-colors ${
          isActive
            ? "bg-blue-600/20 text-blue-400 font-medium"
            : "text-gray-400 hover:text-gray-200 hover:bg-gray-800"
        }`
      }
    >
      {item.icon}
      {item.label}
    </NavLink>
  );

  return (
    <aside className="w-56 flex-shrink-0 bg-gray-900 border-r border-gray-800 flex flex-col h-full">
      <div className="px-4 py-4 border-b border-gray-800">
        <div className="flex items-center gap-2">
          <img src="/icon.svg" alt="" className="w-7 h-7 flex-shrink-0" />
          <div>
            <h1 className="text-base font-bold text-gray-50 tracking-tight">Resty Desktop</h1>
          </div>
        </div>
      </div>

      <nav className="flex-1 px-2 py-3 space-y-0.5 overflow-y-auto">
        {/* slice(0, 3) / slice(3): the non-route Activity entry sits between "Schedules"
            (index 2) and "Logs" (index 3) — see the button below. */}
        {navItems.slice(0, 3).map(renderNavLink)}
        <button
          onClick={onToggleActivity}
          data-activity-toggle
          aria-expanded={activityOpen}
          aria-label={
            activityCount > 0
              ? `Activity, ${activityCount} background ${activityCount === 1 ? "task" : "tasks"} running`
              : "Activity"
          }
          title="Activity"
          className={`flex w-full items-center gap-3 px-3 py-2 rounded-lg text-sm transition-colors ${
            activityOpen
              ? "bg-gray-800 text-gray-200"
              : "text-gray-400 hover:text-gray-200 hover:bg-gray-800"
          }`}
        >
          <svg className="w-5 h-5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
              d="M2 12h4l2.5-6 4 12 2.5-6h5" />
          </svg>
          <span className="truncate">Activity</span>
          {activityCount > 0 && (
            <span
              aria-hidden="true"
              className="ml-auto min-w-[20px] h-5 px-1.5 rounded-full bg-blue-600 text-white text-[11px] font-semibold flex items-center justify-center flex-shrink-0"
            >
              {activityCount}
            </span>
          )}
        </button>
        {navItems.slice(3).map(renderNavLink)}
        {onLock && (
          <button
            onClick={onLock}
            title="Lock Now"
            aria-label="Lock Now"
            className="flex w-full items-center gap-3 px-3 py-2 rounded-lg text-sm text-gray-400 hover:text-gray-200 hover:bg-gray-800 transition-colors"
          >
            <LockIcon className="w-5 h-5" />
            Lock
          </button>
        )}
      </nav>

      {appVersion && (
        <div className="px-4 py-3 border-t border-gray-800">
          <p className="text-xs text-gray-500 text-center">VER {appVersion}</p>
        </div>
      )}
    </aside>
  );
}
