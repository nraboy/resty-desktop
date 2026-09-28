// Minimum supported restic version. Bump as needed when new features are required.
export const MIN_RESTIC_MAJOR = 0;
export const MIN_RESTIC_MINOR = 17;

// `restic backup --pack-size` bounds in MiB. Mirrors MIN/MAX_PACK_SIZE_MIB in
// src-tauri/src/commands/snapshot.rs — restic (verified on 0.19) fails the backup outright outside this range.
export const MIN_PACK_SIZE_MIB = 4;
export const MAX_PACK_SIZE_MIB = 128;
