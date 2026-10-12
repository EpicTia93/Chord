/* eslint-disable no-console -- Forward lifecycle messages to the dev terminal. */
import { dlopen, FFIType } from "bun:ffi";
import {
  chmodSync,
  closeSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  renameSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

// Bun has no filesystem locking API. Use the same BSD flock locks as Rust's File::lock_shared.
const system = dlopen("/usr/lib/libSystem.B.dylib", {
  flock: { args: [FFIType.i32, FFIType.i32], returns: FFIType.i32 },
});
const LOCK_SH = 1;
const LOCK_EX = 2;
const LOCK_NB = 4;

export function lockFile(path: string, exclusive = false, nonblocking = false): number | undefined {
  const fd = openSync(path, "a+", 0o600);
  if (
    system.symbols.flock(fd, (exclusive ? LOCK_EX : LOCK_SH) | (nonblocking ? LOCK_NB : 0)) === 0
  ) {
    return fd;
  }
  closeSync(fd);
  if (nonblocking) return undefined;
  throw new Error(`Cannot lock ${path}`);
}

async function command(args: string[]): Promise<string> {
  const child = Bun.spawn(args, { stdout: "pipe", stderr: "pipe", timeout: 120_000 });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (status !== 0) throw new Error(`${args[0]} failed (${status}): ${stderr.trim()}`);
  return stdout;
}

type Config = {
  identifier: string;
  productName: string;
  version: string;
  bundle: {
    macOS: {
      signingIdentity?: string;
      frameworks?: string[];
      entitlements?: string;
      infoPlist?: string;
    };
  };
};

function merge(
  base: Record<string, unknown>,
  override: Record<string, unknown>,
): Record<string, unknown> {
  const result = { ...base };
  for (const [key, value] of Object.entries(override)) {
    const previous = result[key];
    result[key] =
      value &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      previous &&
      typeof previous === "object" &&
      !Array.isArray(previous)
        ? merge(previous as Record<string, unknown>, value as Record<string, unknown>)
        : value;
  }
  return result;
}

export async function developmentConfig(srcTauri: string): Promise<Config> {
  const base = await Bun.file(join(srcTauri, "tauri.conf.json")).json();
  const dev = await Bun.file(join(srcTauri, "tauri.dev.conf.json")).json();
  const config = merge(merge(base, dev), JSON.parse(process.env.TAURI_CONFIG ?? "{}")) as Config;
  if (!/^com\.leonsilicon\.chord\.development(?:\.[\w-]+)*$/.test(config.identifier)) {
    throw new Error("The snapshot runner only supports Chord development identifiers");
  }
  if (!config.productName || /[/\\\0]/.test(config.productName) || config.productName === "..") {
    throw new Error("Invalid development product name");
  }
  return config;
}

export type Snapshot = { root: string; directory: string; bundle: string; executable: string };

function registryLock(root: string): number {
  mkdirSync(root, { recursive: true, mode: 0o700 });
  return lockFile(join(root, "registry.lock"), true)!;
}

// A directory is reclaimed only under an exclusive lease. Every process running
// its executable holds a shared lease, including detached CLI workers.
export function collectSnapshots(root: string): void {
  const registry = registryLock(root);
  try {
    let current: string | undefined;
    try {
      current = dirname(readlinkSync(join(root, "current")));
    } catch {
      /* First launch. */
    }
    for (const entry of readdirSync(root, { withFileTypes: true })) {
      if (!entry.isDirectory() || !entry.name.startsWith("run-") || entry.name === current)
        continue;
      const directory = join(root, entry.name);
      const lease = lockFile(join(directory, "runtime.lock"), true, true);
      if (lease === undefined) continue;
      try {
        rmSync(directory, { recursive: true, force: true });
      } finally {
        closeSync(lease);
      }
    }
  } finally {
    closeSync(registry);
  }
}

export function publishSnapshot(snapshot: Snapshot): void {
  const registry = registryLock(snapshot.root);
  try {
    const temporary = join(snapshot.root, `current-${crypto.randomUUID()}`);
    symlinkSync(`${basename(snapshot.directory)}/${basename(snapshot.bundle)}`, temporary);
    renameSync(temporary, join(snapshot.root, "current"));
  } finally {
    closeSync(registry);
  }
}

export async function createSnapshot(
  executable: string,
  srcTauri: string,
  config: Config,
  root = join(homedir(), "Library/Application Support", config.identifier, "runtime-snapshots"),
): Promise<{ snapshot: Snapshot; lease: number }> {
  const identity =
    process.env.CHORD_DEV_SIGNING_IDENTITY ??
    process.env.APPLE_SIGNING_IDENTITY ??
    config.bundle.macOS.signingIdentity;
  if (!identity || identity === "-") {
    throw new Error(
      "Set CHORD_DEV_SIGNING_IDENTITY to an Apple Development signing identity; ad-hoc signing cannot preserve TCC identity",
    );
  }
  const registry = registryLock(root);
  let directory: string;
  let lease: number;
  try {
    directory = mkdtempSync(join(root, "run-"));
    lease = lockFile(join(directory, "runtime.lock"))!;
  } finally {
    closeSync(registry);
  }
  const bundle = join(directory, `${config.productName}.app`);
  const contents = join(bundle, "Contents");
  const target = join(contents, "MacOS/chord");
  try {
    mkdirSync(join(contents, "MacOS"), { recursive: true });
    mkdirSync(join(contents, "Frameworks"));
    mkdirSync(join(contents, "Resources"));
    await Bun.write(target, Bun.file(executable));
    chmodSync(target, 0o755);
    const libraries = config.bundle.macOS.frameworks ?? [];
    for (const library of libraries) {
      const source = resolve(srcTauri, library);
      const destination = join(contents, "Frameworks", basename(source));
      await Bun.write(destination, Bun.file(source));
      await command([
        "/usr/bin/codesign",
        "--force",
        "--timestamp=none",
        "--sign",
        identity,
        destination,
      ]);
    }
    if (!libraries.some((library) => basename(library) === "libbun_embed.dylib")) {
      throw new Error("The development bundle must include libbun_embed.dylib");
    }
    // The build's first rpath points into the mutable rbun checkout. Pin the
    // copied executable to its own runtime before signing it.
    await command([
      "/usr/bin/install_name_tool",
      "-change",
      "@rpath/libbun_embed.dylib",
      "@executable_path/../Frameworks/libbun_embed.dylib",
      target,
    ]);
    const extraPlist = config.bundle.macOS.infoPlist
      ? JSON.parse(
          await command([
            "/usr/bin/plutil",
            "-convert",
            "json",
            "-o",
            "-",
            resolve(srcTauri, config.bundle.macOS.infoPlist),
          ]),
        )
      : {};
    await Bun.write(
      join(contents, "Info.plist"),
      JSON.stringify({
        ...extraPlist,
        CFBundleIdentifier: config.identifier,
        CFBundleName: config.productName,
        CFBundleDisplayName: config.productName,
        CFBundleExecutable: "chord",
        CFBundlePackageType: "APPL",
        CFBundleInfoDictionaryVersion: "6.0",
        CFBundleShortVersionString: config.version,
        CFBundleVersion: config.version,
        CFBundleIconFile: "icon.icns",
        NSPrincipalClass: "NSApplication",
        NSHighResolutionCapable: true,
      }),
    );
    await command(["/usr/bin/plutil", "-convert", "xml1", join(contents, "Info.plist")]);
    await Bun.write(
      join(contents, "Resources/icon.icns"),
      Bun.file(join(srcTauri, "icons/icon.icns")),
    );
    const entitlements = config.bundle.macOS.entitlements;
    await command([
      "/usr/bin/codesign",
      "--force",
      "--timestamp=none",
      "--sign",
      identity,
      ...(entitlements ? ["--entitlements", resolve(srcTauri, entitlements)] : []),
      bundle,
    ]);
    await command(["/usr/bin/codesign", "--verify", "--deep", "--strict", bundle]);
    return { snapshot: { root, directory, bundle, executable: realpathSync(target) }, lease };
  } catch (error) {
    closeSync(lease);
    rmSync(directory, { recursive: true, force: true });
    throw error;
  }
}

// Signal handlers must send TERM before yielding: a terminal hangup can close
// the supervisor before async file/child-process checks finish. Bun has no
// synchronous file-content API, so this shutdown-only path uses readFileSync.
export function signalSnapshot(snapshot: Snapshot): void {
  let value: string;
  try {
    value = readFileSync(join(snapshot.directory, "launch.pid"), "utf8");
    if (value.trim() === "-1") value = readFileSync(join(snapshot.directory, "app.pid"), "utf8");
  } catch {
    try {
      value = readFileSync(join(snapshot.directory, "app.pid"), "utf8");
    } catch {
      return;
    }
  }
  const pid = Number(value.trim());
  if (!value.trim()) return;
  if (!Number.isSafeInteger(pid) || pid <= 1) throw new Error("Invalid development app PID");
  const processInfo = Bun.spawnSync(["/bin/ps", "-p", String(pid), "-o", "comm="], {
    stderr: "ignore",
  });
  if (processInfo.exitCode !== 0 || processInfo.stdout.toString().trim() !== snapshot.executable)
    return;
  try {
    process.kill(pid, "SIGTERM");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
  }
}

export function signalSession(stateFile: string): void {
  let snapshot: Snapshot;
  try {
    snapshot = JSON.parse(readFileSync(stateFile, "utf8"));
  } catch {
    return;
  }
  signalSnapshot(snapshot);
}

export async function stopSnapshot(snapshot: Snapshot): Promise<void> {
  const launched = Bun.file(join(snapshot.directory, "launch.pid"));
  let pidFile = (await launched.exists())
    ? launched
    : Bun.file(join(snapshot.directory, "app.pid"));
  if ((await pidFile.exists()) && (await pidFile.text()).trim() === "-1") {
    pidFile = Bun.file(join(snapshot.directory, "app.pid"));
  }
  if (!(await pidFile.exists())) return;
  const value = (await pidFile.text()).trim();
  if (!value) return;
  const pid = Number(value);
  if (!Number.isSafeInteger(pid) || pid <= 1) throw new Error("Invalid development app PID");
  const isOurProcess = async () => {
    const child = Bun.spawn(["/bin/ps", "-p", String(pid), "-o", "comm="], {
      stdout: "pipe",
      stderr: "ignore",
    });
    const output = (await new Response(child.stdout).text()).trim();
    return (await child.exited) === 0 && output === snapshot.executable;
  };
  if (!(await isOurProcess())) return;
  const signal = (name: NodeJS.Signals) => {
    try {
      process.kill(pid, name);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
    }
  };
  signal("SIGTERM");
  const deadline = Date.now() + 2_000;
  while (Date.now() < deadline && (await isOurProcess())) await Bun.sleep(25);
  if (await isOurProcess()) signal("SIGKILL");
}

export async function stopSession(stateFile: string): Promise<void> {
  const file = Bun.file(stateFile);
  if (!(await file.exists())) return;
  const snapshot: Snapshot = await file.json();
  const deadline = Date.now() + 15_000;
  while (!(await Bun.file(join(snapshot.directory, "app.pid")).exists())) {
    if (await Bun.file(join(snapshot.directory, "launch.pid")).exists()) break;
    if (Date.now() >= deadline) return;
    await Bun.sleep(25);
  }
  await stopSnapshot(snapshot);
}

export async function runSnapshot(args: string[]): Promise<number> {
  const [executable, ...appArgs] = args;
  if (!executable) throw new Error("Cargo did not supply an executable");
  const srcTauri = Bun.fileURLToPath(
    new URL("../../../../../../../apps/chord-tauri/src-tauri/", import.meta.url),
  );
  const config = await developmentConfig(srcTauri);
  const { snapshot, lease } = await createSnapshot(resolve(executable), srcTauri, config);
  let open: ReturnType<typeof Bun.spawn> | undefined;
  let stopping = false;
  const stop = () => {
    stopping = true;
    try {
      signalSnapshot(snapshot);
    } catch (error) {
      console.error(`[dev-runtime] ${error}`);
    }
    void stopSnapshot(snapshot).catch((error) =>
      console.error(`[dev-runtime] Failed to stop app: ${error}`),
    );
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
  process.on("SIGHUP", stop);
  const readers: ReturnType<typeof Bun.spawn>[] = [];
  const isStopping = async () =>
    stopping ||
    !!(
      process.env.CHORD_DEV_RUNTIME_STOP_FILE &&
      (await Bun.file(process.env.CHORD_DEV_RUNTIME_STOP_FILE).exists())
    );
  try {
    const launcher = join(snapshot.directory, "launch-app");
    await command([
      "/usr/bin/xcrun",
      "swiftc",
      Bun.fileURLToPath(import.meta.resolve("#launch-app")),
      "-o",
      launcher,
    ]);
    const stateFile = process.env.CHORD_DEV_RUNTIME_STATE_FILE;
    if (stateFile) {
      const temporary = `${stateFile}.tmp`;
      await Bun.write(temporary, JSON.stringify(snapshot));
      renameSync(temporary, stateFile);
    }
    for (const stream of ["stdout", "stderr"] as const) {
      const fifo = join(snapshot.directory, stream);
      await command(["/usr/bin/mkfifo", "-m", "600", fifo]);
      readers.push(
        Bun.spawn(["/bin/cat", fifo], {
          stdout: stream === "stdout" ? "inherit" : 2,
          stderr: "inherit",
        }),
      );
    }
    if (await isStopping()) return 0;
    console.log(`[dev-runtime] Launching ${snapshot.bundle}`);
    // NSWorkspace passes the environment directly, without exposing values in
    // argv. Chord connects its inherited log streams at the start of main().
    open = Bun.spawn([launcher, snapshot.bundle, ...appArgs], {
      env: { ...process.env, CHORD_DEV_STDIO_DIRECTORY: snapshot.directory },
      stdout: "inherit",
      stderr: "inherit",
    });
    const timeout = Date.now() + 15_000;
    while (!(await Bun.file(join(snapshot.directory, "app.pid")).exists())) {
      if (open.exitCode !== null)
        throw new Error(`Development app exited before startup (${open.exitCode})`);
      if (Date.now() >= timeout)
        throw new Error("Development app did not report its PID within 15 seconds");
      await Bun.sleep(25);
    }
    publishSnapshot(snapshot);
    collectSnapshots(snapshot.root);
    if (await isStopping()) await stopSnapshot(snapshot);
    const status = await open.exited;
    if (status !== 0 || (await isStopping())) return status;
    if (await Bun.file(join(snapshot.directory, "handoff")).exists()) return 0;
    const result = Bun.file(join(snapshot.directory, "app.exit"));
    if (!(await result.exists()))
      throw new Error("Development app terminated without reporting a successful exit");
    const appStatus = Number((await result.text()).trim());
    if (!Number.isInteger(appStatus) || appStatus < 0 || appStatus > 255)
      throw new Error("Invalid development app exit status");
    return appStatus;
  } finally {
    await stopSnapshot(snapshot);
    if (open?.exitCode === null) open.kill();
    for (const reader of readers) if (reader.exitCode === null) reader.kill();
    process.off("SIGINT", stop);
    process.off("SIGTERM", stop);
    process.off("SIGHUP", stop);
    closeSync(lease);
    collectSnapshots(snapshot.root);
  }
}
