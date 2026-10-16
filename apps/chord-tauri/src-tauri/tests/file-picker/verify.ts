// Keeps the existing entry point; the fixture and listener are both Rust.
const manifest = Bun.fileURLToPath(new URL("../../Cargo.toml", import.meta.url));
const child = Bun.spawn([
  "cargo", "run", "--manifest-path", manifest, "--example", "file-picker",
  ...(process.argv.includes("--ui") ? ["--", "--ui"] : []),
], { stdout: "inherit", stderr: "inherit", timeout: 300_000 });
process.exitCode = await child.exited;
