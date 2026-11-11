declare const process: { env: Record<string, string | undefined> };
declare const __dirname: string;
declare module "node:path" { export function resolve(...parts: string[]): string; }
