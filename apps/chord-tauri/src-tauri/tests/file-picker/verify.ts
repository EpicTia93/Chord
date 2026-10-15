import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const root = Bun.fileURLToPath(new URL("../../", import.meta.url));
const directory = await mkdtemp(join(tmpdir(), "chord-file-picker-"));

async function run(command: string[]) {
  const child = Bun.spawn(command, {
    cwd: root,
    stdout: "inherit",
    stderr: "inherit",
    timeout: 60_000,
  });
  const code = await child.exited;
  if (code !== 0) throw new Error(`${command[0]} exited with ${code}`);
}

try {
  const executable = join(directory, "listener");
  await run([
    "xcrun",
    "--sdk",
    "macosx",
    "clang",
    "-fobjc-arc",
    "-fblocks",
    "-Wall",
    "-Wextra",
    "-Werror",
    "-Wno-unused-parameter",
    "-framework",
    "AppKit",
    "-framework",
    "ApplicationServices",
    "native/file_picker.m",
    "tests/file-picker/listener.m",
    "-o",
    executable,
  ]);
  await run([executable, "--identifiers"]);
  if (process.argv.includes("--ui")) await run([executable]);
} finally {
  await rm(directory, { recursive: true, force: true });
}
