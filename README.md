# Restic GUI

A cross-platform desktop client for [Restic](https://restic.net/), the fast and secure backup tool. Restic GUI wraps the Restic CLI to provide a visual interface for managing repositories, creating backups, browsing snapshots, and restoring files — without touching the command line.

## Features

- **Repository management** — add local or remote repositories (S3, SFTP, B2, etc.), initialize new ones, check integrity, and rename or remove them
- **Backups** — define backup plans with source paths, tags, and exclude patterns; run plans on demand
- **Snapshots** — browse all snapshots in a repository, filter by host/path/tag, add or remove tags, and delete with optional pruning
- **File browser** — navigate the file tree inside any snapshot and restore individual files or directories
- **Retention policies** — configure keep-last/daily/weekly/monthly/yearly rules per backup plan

## Requirements

- [Restic](https://restic.readthedocs.io/en/latest/020_installation.html) installed and available on `$PATH` (or configured via Settings)
- [Rust](https://rustup.rs/) (for building from source)
- Node.js 18+

## Getting Started

Install Rust if you haven't already:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then install dependencies and start the development build:

```bash
npm install
npm run tauri dev
```

## Building a Distributable

```bash
npm run tauri build
```

The packaged app will be written to `src-tauri/target/release/bundle/`.

## Stack

| Layer | Choice |
|---|---|
| Desktop shell | Tauri v2 |
| Frontend | React 19 + TypeScript |
| Styling | Tailwind CSS v3 |
| Build tool | Vite |
| Routing | React Router v6 |
| Rust backend | Tauri v2 `#[tauri::command]` |
| Settings persistence | `tauri-plugin-store` |
| Restic integration | `std::process::Command` with `--json` flag |

## Configuration

The Restic binary path defaults to `restic` on `$PATH`. You can override it in the Settings page if Restic is installed elsewhere.

Repository passwords and settings are stored locally in `settings.json` via `tauri-plugin-store`.
