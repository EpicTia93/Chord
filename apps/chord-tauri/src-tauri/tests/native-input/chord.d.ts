// Chord's built-in `chord` module, as far as verify.ts uses it. Options stay loose
// because the script passes invalid values on purpose to check the bridge rejects them.
declare module "chord" {
  export function getAppWindows(bundleId: string): unknown[];
  export function click(options: Record<string, unknown>): Promise<void>;
}
