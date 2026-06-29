export function needsFullDiskAccess(p: string): boolean {
  return (
    /\/Library(\/|$)/.test(p) ||
    p === "/System" || p.startsWith("/System/") ||
    p === "/private" || p.startsWith("/private/") ||
    p === "/var" || p.startsWith("/var/")
  );
}
