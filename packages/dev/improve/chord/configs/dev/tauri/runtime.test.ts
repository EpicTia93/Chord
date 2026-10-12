import { describe, expect, test } from "bun:test";
import { closeSync, existsSync, mkdirSync, mkdtempSync, readlinkSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const runtime = process.platform === "darwin" ? await import("#runtime") : undefined;

describe.skipIf(!runtime)("macOS development runtime", () => {
  test("collection preserves both current and leased generations", async () => {
    const root = mkdtempSync(join(tmpdir(), "chord-runtime-test-"));
    const active = join(root, "run-active");
    const current = join(root, "run-current");
    const unused = join(root, "run-unused");
    for (const directory of [active, current, unused]) mkdirSync(directory);
    const activeLease = runtime!.lockFile(join(active, "runtime.lock"))!;
    const inactiveLease = runtime!.lockFile(join(unused, "runtime.lock"))!;
    closeSync(inactiveLease);
    runtime!.publishSnapshot({
      root,
      directory: current,
      bundle: join(current, "Chord.app"),
      executable: "unused",
    });
    try {
      runtime!.collectSnapshots(root);
      expect(existsSync(active)).toBe(true);
      expect(existsSync(current)).toBe(true);
      expect(existsSync(unused)).toBe(false);
      closeSync(activeLease);
      runtime!.collectSnapshots(root);
      expect(existsSync(active)).toBe(false);
      expect(readlinkSync(join(root, "current"))).toBe("run-current/Chord.app");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("signed snapshots keep their executable and Bun library through a source rebuild", async () => {
    const directory = mkdtempSync(join(tmpdir(), "chord-runtime-build-test-"));
    let lease: number | undefined;
    const run = async (args: string[]) => {
      const process = Bun.spawn(args, { stdout: "pipe", stderr: "pipe" });
      const [stdout, stderr, status] = await Promise.all([
        new Response(process.stdout).text(),
        new Response(process.stderr).text(),
        process.exited,
      ]);
      if (status !== 0) throw new Error(`${args[0]}: ${stderr}`);
      return stdout;
    };
    try {
      await Bun.write(join(directory, "bun.c"), "int fixture_version(void) { return 42; }\n");
      await Bun.write(
        join(directory, "app.c"),
        '#include <stdio.h>\nextern int fixture_version(void);\nint main(void) { printf("%d\\n", fixture_version()); return 0; }\n',
      );
      const library = join(directory, "libbun_embed.dylib");
      const executable = join(directory, "chord");
      await run([
        "clang",
        "-dynamiclib",
        join(directory, "bun.c"),
        "-install_name",
        "@rpath/libbun_embed.dylib",
        "-o",
        library,
      ]);
      await run([
        "clang",
        join(directory, "app.c"),
        `-L${directory}`,
        "-lbun_embed",
        `-Wl,-rpath,${directory}`,
        "-o",
        executable,
      ]);
      const srcTauri = Bun.fileURLToPath(
        new URL("../../../../../../../apps/chord-tauri/src-tauri/", import.meta.url),
      );
      const config = await runtime!.developmentConfig(srcTauri);
      config.bundle.macOS.frameworks = [library];
      const created = await runtime!.createSnapshot(
        executable,
        srcTauri,
        config,
        join(directory, "snapshots"),
      );
      lease = created.lease;
      await Bun.write(executable, "replaced by another build");
      await Bun.write(library, "replaced by an rbun rebuild");
      expect((await run([created.snapshot.executable])).trim()).toBe("42");
      await run(["codesign", "--verify", "--deep", "--strict", created.snapshot.bundle]);
      expect(await run(["codesign", "-d", "-r-", created.snapshot.bundle])).toContain(
        'identifier "com.leonsilicon.chord.development"',
      );
    } finally {
      if (lease !== undefined) closeSync(lease);
      rmSync(directory, { recursive: true, force: true });
    }
  }, 60_000);

  test("signal forwarding refuses a PID belonging to another executable", async () => {
    const directory = mkdtempSync(join(tmpdir(), "chord-runtime-pid-test-"));
    const child = Bun.spawn(["/bin/sleep", "30"]);
    try {
      await Bun.write(join(directory, "app.pid"), String(child.pid));
      await runtime!.stopSnapshot({
        root: directory,
        directory,
        bundle: "unused",
        executable: "/different/executable",
      });
      expect(child.exitCode).toBe(null);
    } finally {
      child.kill();
      await child.exited;
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test("Launch Services logs and shutdown survive a supervisor signal", async () => {
    const directory = mkdtempSync(join(tmpdir(), "chord-runtime-launch-test-"));
    const stateFile = join(directory, "state.json");
    let child: Bun.Subprocess<"ignore", "pipe", "pipe"> | undefined;
    let snapshot: import("#runtime").Snapshot | undefined;
    const run = async (args: string[]) => {
      const result = Bun.spawnSync(args, { stderr: "pipe" });
      if (result.exitCode !== 0) throw new Error(result.stderr.toString());
    };
    try {
      await Bun.write(join(directory, "bun.c"), "int fixture_version(void) { return 42; }\n");
      await Bun.write(
        join(directory, "app.m"),
        `
#import <AppKit/AppKit.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/file.h>
#include <unistd.h>
extern int fixture_version(void);
int main(void) {
  @autoreleasepool {
    const char *directory = getenv("CHORD_DEV_STDIO_DIRECTORY");
    char path[4096];
    snprintf(path, sizeof(path), "%s/runtime.lock", directory);
    int lease = open(path, O_RDWR); flock(lease, LOCK_SH);
    snprintf(path, sizeof(path), "%s/stdout", directory);
    dup2(open(path, O_WRONLY), 1);
    snprintf(path, sizeof(path), "%s/stderr", directory);
    dup2(open(path, O_WRONLY), 2);
    NSApplication *app = [NSApplication sharedApplication];
    [app finishLaunching];
    printf("fixture output: %d\\n", fixture_version()); fflush(stdout);
    snprintf(path, sizeof(path), "%s/app.pid.tmp", directory);
    FILE *pid = fopen(path, "w"); fprintf(pid, "%d\\n", getpid()); fclose(pid);
    char published[4096]; snprintf(published, sizeof(published), "%s/app.pid", directory);
    rename(path, published);
    [app run];
  }
  return 0;
}`,
      );
      const library = join(directory, "libbun_embed.dylib");
      const executable = join(directory, "chord");
      await run([
        "clang",
        "-dynamiclib",
        join(directory, "bun.c"),
        "-install_name",
        "@rpath/libbun_embed.dylib",
        "-o",
        library,
      ]);
      await run([
        "clang",
        join(directory, "app.m"),
        "-framework",
        "AppKit",
        `-L${directory}`,
        "-lbun_embed",
        `-Wl,-rpath,${directory}`,
        "-o",
        executable,
      ]);
      child = Bun.spawn(
        [process.execPath, Bun.fileURLToPath(import.meta.resolve("#runtime-launch")), executable],
        {
          env: {
            ...process.env,
            CHORD_DEV_RUNTIME_STATE_FILE: stateFile,
            CHORD_DEV_RUNTIME_STOP_FILE: join(directory, "stop"),
            TAURI_CONFIG: JSON.stringify({
              identifier: `com.leonsilicon.chord.development.fixture-${crypto.randomUUID()}`,
              productName: "Chord Runtime Fixture",
              bundle: { macOS: { frameworks: [library], infoPlist: null } },
            }),
          },
        stdin: "ignore",
        stdout: "pipe",
          stderr: "pipe",
        },
      );
      const stdout = new Response(child.stdout).text();
      const stderr = new Response(child.stderr).text();
      const deadline = Date.now() + 20_000;
      while (Date.now() < deadline) {
        if (existsSync(stateFile)) {
          snapshot = await Bun.file(stateFile).json();
          if (existsSync(join(snapshot!.directory, "app.pid"))) break;
        }
        if (child.exitCode !== null) throw new Error(await stderr);
        await Bun.sleep(25);
      }
      expect(snapshot).toBeDefined();
      const pid = Number(await Bun.file(join(snapshot!.directory, "app.pid")).text());
      // Check HUP, which a disappearing PTY can deliver immediately after INT.
      child.kill("SIGHUP");
      expect(await child.exited).toBe(0);
      expect(await stdout).toContain("fixture output: 42");
      expect(await stderr).not.toContain("terminated without reporting");
      expect(Bun.spawnSync(["/bin/ps", "-p", String(pid)]).exitCode).not.toBe(0);
    } finally {
      if (snapshot) await runtime!.stopSnapshot(snapshot);
      if (child?.exitCode === null) {
        child.kill("SIGKILL");
        await child.exited;
      }
      if (snapshot) rmSync(snapshot.root, { recursive: true, force: true });
      rmSync(directory, { recursive: true, force: true });
    }
  }, 60_000);
});
