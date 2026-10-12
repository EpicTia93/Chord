// Reuse a NodeSwift 1.5.1 build: no SwiftSyntax/macro compiler rebuild is needed.
import { mkdir } from "node:fs/promises";

const root = process.argv[2];
if (!root) throw new Error("Pass the NodeSwift SwiftPM build directory as the first argument");
const cwd = Bun.pathToFileURL(process.cwd() + "/");
const swiftpm = new URL(root.replace(/\/$/, "") + "/", cwd);
const build = new URL("debug/", swiftpm);
const output = new URL(
  (process.argv[3] ?? "/tmp/chord-legacy-node-swift-probes").replace(/\/$/, "") + "/",
  cwd,
);
await mkdir(Bun.fileURLToPath(output), { recursive: true });

for (const name of ["First", "Second"]) {
  const compilation = Bun.spawn(
    [
      "swiftc",
      "-swift-version",
      "6",
      "-emit-library",
      "-module-name",
      "ChordLegacy" + name,
      "-I",
      Bun.fileURLToPath(new URL("Modules/", build)),
      "-Xcc",
      "-fmodule-map-file=" +
        Bun.fileURLToPath(
          new URL("checkouts/node-swift/Sources/CNodeAPI/module.modulemap", swiftpm),
        ),
      "-Xcc",
      "-fmodule-map-file=" +
        Bun.fileURLToPath(new URL("CNodeAPISupport.build/module.modulemap", build)),
      "-L",
      Bun.fileURLToPath(build),
      "-lNodeAPI",
      "-Xlinker",
      "-rpath",
      "-Xlinker",
      Bun.fileURLToPath(build),
      Bun.fileURLToPath(new URL(name + ".swift", import.meta.url)),
      "-o",
      Bun.fileURLToPath(new URL(name + ".node", output)),
    ],
    { stdout: "inherit", stderr: "inherit", timeout: 60_000 },
  );
  const status = await compilation.exited;
  if (status !== 0) throw new Error(name + " compilation failed (" + status + ")");
}
console.log("CHORD_LEGACY_NATIVE_FIXTURE=" + Bun.fileURLToPath(output));
