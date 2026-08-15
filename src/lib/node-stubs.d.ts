// Minimal ambient declarations for the node built-ins used by contrast.test.ts.
// The project deliberately does not depend on @types/node — vite.config.ts carries a
// @ts-expect-error on `process` precisely because it isn't in the type graph, and
// installing it would turn that suppression into a hard typecheck error. These stubs
// give the test file just enough typing; at runtime Vitest (node environment)
// resolves the real modules.
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf-8"): string;
}
declare module "node:url" {
  export function fileURLToPath(url: string): string;
}
declare module "node:path" {
  export function dirname(p: string): string;
  export function join(...paths: string[]): string;
}
